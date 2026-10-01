//! A program with its checker pool (docs/PHASE2-C6-plan.md, C6.4): the pin's
//! `Program` fields `checkerPool` and `compilerCheckerPool` and the program
//! functions that drive them. They sit beside the immutable [`Program`]
//! rather than in it because the pool's checkers own the program through
//! their host.
//!
//! Every Rust checker access holds the checker's operation, so the pin's
//! non-exclusive acquisitions, which hand out a checker without its lock and
//! rely on the caller, hold it here for the task they serve; the emit
//! resolver of a file's declaration transform holds the file's checker for
//! that transform, which is one of the interleavings the pin's per-call locks
//! allow.
//!
//! Where the pin's program functions take a context, these take a
//! [`CheckerRequest`]: its lifetime selects the checker a supplied pool serves
//! (the compiler's pool has no lifetimes) and its token cancels the checks.
use crate::{CompilerCheckerPool, Error, Program, ProgramFile};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use tsr_arena::{Counters, NodeId};
use tsr_ast::Diagnostic;
use tsr_checker::{CheckerPool, CheckerRequest, Operation, TraceSink};
use tsr_core::workgroup::WorkGroup;

/// A per-file collection for the checker: it receives the file's checker.
pub type CheckerCollect<'a, R> = dyn Fn(&mut Operation<'_>, &ProgramFile) -> R + Sync + 'a;

pub struct CheckedProgram {
    program: Arc<Program>,
    pool: Arc<dyn CheckerPool>,
    compiler_pool: Option<Arc<CompilerCheckerPool>>,
    /// The program's trace session (`p.opts.Tracing`), which `Program.Emit`
    /// pushes its events to; the compiler's pool traces its checkers with it.
    tracing: Option<Arc<dyn TraceSink>>,
}

impl CheckedProgram {
    /// The program with the compiler's pool, optionally tracing its checkers.
    // port: tsc/internal/compiler/program.go:Program.initCheckerPool
    pub fn new(
        program: Arc<Program>,
        counters: &Counters,
        tracing: Option<Arc<dyn TraceSink>>,
    ) -> Self {
        let pool = Arc::new(CompilerCheckerPool::with_tracing(
            program.clone(),
            counters,
            tracing.clone(),
        ));
        Self {
            program,
            pool: pool.clone(),
            compiler_pool: Some(pool),
            tracing,
        }
    }

    /// The program with a pool the caller supplies (the pin's
    /// `CreateCheckerPool`), such as the project system's.
    pub fn with_pool(program: Arc<Program>, pool: Arc<dyn CheckerPool>) -> Self {
        Self {
            program,
            pool,
            compiler_pool: None,
            tracing: None,
        }
    }

    pub fn program(&self) -> &Arc<Program> {
        &self.program
    }

    /// The program's trace session, if it traces.
    pub fn tracing(&self) -> Option<&Arc<dyn TraceSink>> {
        self.tracing.as_ref()
    }

    // port: tsc/internal/compiler/program.go:Program.GetCheckerPool
    pub fn checker_pool(&self) -> &Arc<dyn CheckerPool> {
        &self.pool
    }

    /// The compiler's pool, when it is the program's pool.
    pub fn compiler_checker_pool(&self) -> Option<&Arc<CompilerCheckerPool>> {
        self.compiler_pool.as_ref()
    }

    /// Runs `task` with the program's first checker.
    // port: tsc/internal/compiler/program.go:Program.GetTypeChecker
    pub fn with_type_checker(
        &self,
        request: &CheckerRequest,
        task: &mut dyn FnMut(&mut Operation<'_>) -> Result<(), tsr_checker::Error>,
    ) -> Result<(), tsr_checker::Error> {
        match &self.compiler_pool {
            Some(pool) => task(&mut pool.checker_non_exclusive()?.operation()?),
            None => self.pool.with_checker(request.lifetime, None, task),
        }
    }

    /// Runs `task` with the checker that checks `file`. Only non-type data
    /// may leave it: types of different checkers do not mix.
    // port: tsc/internal/compiler/program.go:Program.GetTypeCheckerForFile
    pub fn with_type_checker_for_file(
        &self,
        request: &CheckerRequest,
        file: NodeId,
        task: &mut dyn FnMut(&mut Operation<'_>) -> Result<(), tsr_checker::Error>,
    ) -> Result<(), tsr_checker::Error> {
        match &self.compiler_pool {
            Some(pool) => task(&mut pool.checker_for_file_non_exclusive(file)?.operation()?),
            None => self.pool.with_checker(request.lifetime, Some(file), task),
        }
    }

    /// Runs `task` holding the checker that checks `file`.
    // port: tsc/internal/compiler/program.go:Program.GetTypeCheckerForFileExclusive
    pub fn with_type_checker_for_file_exclusive(
        &self,
        request: &CheckerRequest,
        file: NodeId,
        task: &mut dyn FnMut(&mut Operation<'_>) -> Result<(), tsr_checker::Error>,
    ) -> Result<(), tsr_checker::Error> {
        match &self.compiler_pool {
            Some(pool) => task(&mut pool.checker_for_file_exclusive(file)?),
            None => self.pool.with_checker(request.lifetime, Some(file), task),
        }
    }

    /// Every checker of the compiler's pool at once; nothing with another
    /// pool.
    // port: tsc/internal/compiler/program.go:Program.ForEachCheckerParallel
    pub fn for_each_checker_parallel(
        &self,
        task: &(dyn Fn(usize, &mut Operation<'_>) + Sync),
    ) -> Result<(), tsr_checker::Error> {
        match &self.compiler_pool {
            Some(pool) => pool.for_each_checker_parallel(task),
            None => Ok(()),
        }
    }

    /// `collect` for every file, in a work group that is single-threaded
    /// (last queued first) unless `concurrent` and the program is not;
    /// results in file order.
    // port: tsc/internal/compiler/program.go:Program.collectDiagnosticsFromFiles
    pub fn collect_diagnostics_from_files<R: Send>(
        &self,
        files: &[Arc<ProgramFile>],
        concurrent: bool,
        collect: &(dyn Fn(&ProgramFile) -> R + Sync),
    ) -> Vec<R> {
        let results: Vec<Mutex<Option<R>>> = files.iter().map(|_| Mutex::new(None)).collect();
        let group = WorkGroup::new(!concurrent || self.program.single_threaded());
        for (file, slot) in files.iter().zip(&results) {
            group.queue(move || {
                *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(collect(file));
            });
        }
        group.run_and_wait();
        drop(group);
        results
            .into_iter()
            .map(|slot| {
                slot.into_inner()
                    .unwrap_or_else(PoisonError::into_inner)
                    .expect("every queued collection ran")
            })
            .collect()
    }

    /// `collect` for every file with the file's checker. With the compiler's
    /// pool one task per checker visits its files in program order; with
    /// another pool each file not skipped by type checking acquires its
    /// checker in its own task, and a skipped file has no result.
    // port: tsc/internal/compiler/program.go:Program.collectCheckerDiagnosticsFromFiles
    pub fn collect_checker_diagnostics_from_files<R: Send>(
        &self,
        request: &CheckerRequest,
        files: &[Arc<ProgramFile>],
        collect: &CheckerCollect<'_, R>,
    ) -> Result<Vec<Option<R>>, Error> {
        let results: Vec<Mutex<Option<R>>> = files.iter().map(|_| Mutex::new(None)).collect();
        if let Some(pool) = &self.compiler_pool {
            let sources: Vec<NodeId> = files.iter().map(|file| file.source()).collect();
            pool.for_each_checker_group_do(
                &sources,
                self.program.single_threaded(),
                &|operation, position, _| {
                    let value = collect(operation, &files[position]);
                    *results[position]
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner) = Some(value);
                },
            )?;
        } else {
            let failure = Mutex::new(None);
            let group = WorkGroup::new(self.program.single_threaded());
            for (file, slot) in files.iter().zip(&results) {
                if self.program.skip_type_checking(file, false)? {
                    continue;
                }
                let failure = &failure;
                group.queue(move || {
                    let served = self.pool.with_checker(
                        request.lifetime,
                        Some(file.source()),
                        &mut |operation| {
                            *slot.lock().unwrap_or_else(PoisonError::into_inner) =
                                Some(collect(operation, file));
                            Ok(())
                        },
                    );
                    if let Err(error) = served {
                        failure
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .get_or_insert(error);
                    }
                });
            }
            group.run_and_wait();
            drop(group);
            if let Some(error) = failure.into_inner().unwrap_or_else(PoisonError::into_inner) {
                return Err(error.into());
            }
        }
        Ok(results
            .into_iter()
            .map(|slot| slot.into_inner().unwrap_or_else(PoisonError::into_inner))
            .collect())
    }

    /// One file's diagnostics with its checker held, or every file's; either
    /// way filtered, sorted and deduplicated.
    // port: tsc/internal/compiler/program.go:Program.collectCheckerDiagnostics
    fn collect_checker_diagnostics(
        &self,
        request: &CheckerRequest,
        file: Option<&ProgramFile>,
        collect: &CheckerCollect<'_, Result<Vec<Diagnostic>, Error>>,
    ) -> Result<Vec<Diagnostic>, Error> {
        let diagnostics = if let Some(file) = file {
            if self.program.skip_type_checking(file, false)? {
                return Ok(Vec::new());
            }
            let mut result = None;
            self.with_type_checker_for_file_exclusive(request, file.source(), &mut |operation| {
                result = Some(collect(operation, file));
                Ok(())
            })?;
            result.expect("the task ran")?
        } else {
            let mut diagnostics = Vec::new();
            for result in self
                .collect_checker_diagnostics_from_files(request, self.program.files(), collect)?
                .into_iter()
                .flatten()
            {
                diagnostics.extend(result?);
            }
            diagnostics
        };
        self.program.filter_and_sort_diagnostics(&diagnostics)
    }

    // port: tsc/internal/compiler/program.go:Program.GetSemanticDiagnostics
    pub fn semantic_diagnostics(
        &self,
        request: &CheckerRequest,
        file: Option<&ProgramFile>,
    ) -> Result<Vec<Diagnostic>, Error> {
        let cancellation = request.cancellation.as_ref();
        self.collect_checker_diagnostics(request, file, &|operation, file| {
            self.program
                .semantic_diagnostics_in(operation, file, cancellation)
        })
    }

    /// Each of `source_files`' bind and checker diagnostics, filtered and
    /// sorted but not filtered for `noEmit`, aligned with `source_files` (the
    /// pin's map from file to diagnostics); a file another pool skips has
    /// none.
    // port: tsc/internal/compiler/program.go:Program.GetSemanticDiagnosticsWithoutNoEmitFiltering
    pub fn semantic_diagnostics_without_no_emit_filtering(
        &self,
        request: &CheckerRequest,
        source_files: &[Arc<ProgramFile>],
    ) -> Result<Vec<Vec<Diagnostic>>, Error> {
        let cancellation = request.cancellation.as_ref();
        let all_diags = self.collect_checker_diagnostics_from_files(
            request,
            source_files,
            &|operation, file| {
                self.program
                    .bind_and_check_diagnostics_in(operation, file, cancellation)
            },
        )?;
        let mut result = Vec::with_capacity(all_diags.len());
        for diags in all_diags {
            let diags = diags.transpose()?.unwrap_or_default();
            result.push(self.program.filter_and_sort_diagnostics(&diags)?);
        }
        Ok(result)
    }

    // port: tsc/internal/compiler/program.go:Program.GetSuggestionDiagnostics
    pub fn suggestion_diagnostics(
        &self,
        request: &CheckerRequest,
        file: Option<&ProgramFile>,
    ) -> Result<Vec<Diagnostic>, Error> {
        let cancellation = request.cancellation.as_ref();
        self.collect_checker_diagnostics(request, file, &|operation, file| {
            self.program
                .suggestion_diagnostics_in(operation, file, cancellation)
        })
    }

    /// One file's declaration diagnostics, or every file's in the
    /// concurrent collection, each with its file's checker.
    // port: tsc/internal/compiler/program.go:Program.GetDeclarationDiagnostics
    pub fn declaration_diagnostics(
        &self,
        request: &CheckerRequest,
        file: Option<&ProgramFile>,
    ) -> Result<Vec<Diagnostic>, Error> {
        let diagnostics = if let Some(file) = file {
            self.declaration_diagnostics_for_file(request, file)?
        } else {
            let mut diagnostics = Vec::new();
            for result in self.collect_diagnostics_from_files(self.program.files(), true, &|file| {
                self.declaration_diagnostics_for_file(request, file)
            }) {
                diagnostics.extend(result?);
            }
            diagnostics
        };
        self.program.filter_and_sort_diagnostics(&diagnostics)
    }

    /// A file's declaration diagnostics with the file's checker as the
    /// emit resolver.
    pub fn declaration_diagnostics_for_file(
        &self,
        request: &CheckerRequest,
        file: &ProgramFile,
    ) -> Result<Vec<Diagnostic>, Error> {
        let mut result = None;
        self.with_type_checker_for_file(request, file.source(), &mut |operation| {
            result = Some(
                self.program
                    .declaration_diagnostics_with_checker(operation, file),
            );
            Ok(())
        })?;
        result.expect("the task ran")
    }

    /// With the compiler's pool, every checker's global diagnostics merged;
    /// another pool reports them as its checkers are used, so none here.
    // port: tsc/internal/compiler/program.go:Program.GetGlobalDiagnostics
    pub fn global_diagnostics(&self) -> Result<Vec<Diagnostic>, Error> {
        if self.program.files().is_empty() {
            return Ok(Vec::new());
        }
        match &self.compiler_pool {
            Some(pool) => pool.global_diagnostics(),
            None => Ok(Vec::new()),
        }
    }
}

/// Operations on a program's checkers for a walk over its files that keeps
/// its results, each file served by the checker that checks it (the pin's
/// `GetTypeCheckerForFile` per file). It holds one operation for every file,
/// or every checker's of the compiler's pool; [`FileCheckers::current_mut`]
/// is the checker of the file last selected.
pub struct FileCheckers<'a, 'op> {
    operations: Vec<&'a mut Operation<'op>>,
    checker_of: HashMap<NodeId, usize>,
    current: usize,
}

impl<'a, 'op> FileCheckers<'a, 'op> {
    /// One checker for every file.
    pub fn single(operation: &'a mut Operation<'op>) -> Self {
        Self {
            operations: vec![operation],
            checker_of: HashMap::new(),
            current: 0,
        }
    }

    /// Every checker of `pool`, whose operations `operations` holds in
    /// checker order.
    pub fn for_pool(
        pool: &CompilerCheckerPool,
        operations: Vec<&'a mut Operation<'op>>,
    ) -> Result<Self, Error> {
        let checkers = pool.checkers()?;
        if operations.len() != checkers.len()
            || operations
                .iter()
                .zip(checkers)
                .any(|(operation, owner)| !Arc::ptr_eq(operation.owner(), owner))
        {
            return Err(tsr_arena::Error::WrongOwner.into());
        }
        let plan = pool.association_plan()?;
        let checker_of = pool
            .program()
            .files()
            .iter()
            .zip(&plan.associations)
            .map(|(file, &checker)| (file.source(), checker))
            .collect();
        Ok(Self {
            operations,
            checker_of,
            current: 0,
        })
    }

    /// Makes `file`'s checker current and returns its index.
    pub fn select(&mut self, file: NodeId) -> Result<usize, Error> {
        if self.operations.len() > 1 {
            self.current = *self
                .checker_of
                .get(&file)
                .ok_or(tsr_arena::Error::WrongOwner)?;
        }
        Ok(self.current)
    }

    pub fn current(&self) -> usize {
        self.current
    }

    /// The operation of the checker of the file last selected.
    pub fn current_mut(&mut self) -> &mut Operation<'op> {
        self.operations[self.current]
    }

    pub fn len(&self) -> usize {
        self.operations.len()
    }

    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }

    /// Every held operation, in checker order.
    pub fn iter(&self) -> impl Iterator<Item = &Operation<'op>> {
        self.operations.iter().map(|operation| &**operation)
    }

    /// The operation of the checker at `index`.
    pub fn checker(&mut self, index: usize) -> &mut Operation<'op> {
        self.operations[index]
    }
}

/// Without `noEmit`, `diagnostics`; with it, those not skipped on `noEmit`.
// port: tsc/internal/compiler/program.go:FilterNoEmitSemanticDiagnostics
pub fn filter_no_emit_semantic_diagnostics(
    diagnostics: Vec<Diagnostic>,
    options: &tsr_core::CompilerOptions,
) -> Vec<Diagnostic> {
    if !options.no_emit.is_true() {
        return diagnostics;
    }
    diagnostics
        .into_iter()
        .filter(|d| !d.skipped_on_no_emit)
        .collect()
}
