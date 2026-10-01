//! An embedding session owns one immutable program and one lazy checker.
//!
//! Supply a filesystem explicitly through [`ProgramOptions`]; an in-memory
//! snapshot requires no OS services. Query results that outlive an operation
//! must use the checker's `retain_*` methods. Such results keep their program
//! alive after the session is dropped. [`Session::retire`] instead invalidates
//! subsequent operations, including operations through retained results.
//!
//! [`Session::emit`] runs the compiler's `Program.Emit` with an in-memory write
//! callback. It carries no acceptance claim: Phase 7 grades it
//! (docs/PHASE3-plan.md, decision 9).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use tsr_arena::{CheckerIdentity, Counters, Generation};
use tsr_ast::{Diagnostic, NodeId};
use tsr_checker::{CheckerOwner, CheckerRequest, Error, Operation};
use tsr_compiler::{CheckedProgram, WriteFile, WriteFileData};
pub use tsr_compiler::{EmitOnly, FileCache, Program, ProgramFile, ProgramOptions};
use tsr_jsstring::JsString;

/// Cheap, explicitly retained root for a loaded program and its checker.
/// Initialization occurs on the first query, under the checker's identity
/// permit. Reentry fails before waiting on the initialization cell. Failed
/// initialization is remembered; a panic retires the generation via the permit.
pub struct Session {
    program: Arc<Program>,
    identity: Arc<CheckerIdentity>,
    counters: Counters,
    checker: OnceLock<Result<Arc<CheckerOwner>, Error>>,
    /// The program with the compiler's checker pool, created by the first
    /// emit. Its checkers are separate from the query checker above.
    emitter: OnceLock<CheckedProgram>,
}

/// The emit options an embedder chooses: the compiler's `EmitOptions`
/// without its `WriteFile`, which the session supplies in memory.
#[derive(Clone, Copy, Debug, Default)]
pub struct EmitOptions<'a> {
    /// Files of the session's program to emit, in this order; `None` emits
    /// every file. Take them from [`Session::program`].
    pub target_source_files: Option<&'a [&'a ProgramFile]>,
    pub emit_only: EmitOnly,
    pub force_emit: bool,
}

/// What one emit produced, owned by the caller.
#[derive(Clone, Debug, Default)]
pub struct EmitOutput {
    /// `EmitResult.EmitSkipped`.
    pub emit_skipped: bool,
    /// `EmitResult.Diagnostics`: declaration emit diagnostics, and with
    /// `noEmitOnError` the program's diagnostics that skipped the emit.
    pub diagnostics: Vec<Diagnostic>,
    /// Every written file, in `EmitResult.EmittedFiles` order.
    pub files: Vec<EmittedFile>,
}

/// One written output: its name as the emitter wrote it and its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmittedFile {
    pub name: JsString,
    pub text: Vec<u8>,
}

impl Session {
    /// Load through the supplied host. The caller controls cache reuse and its
    /// lifetime. Its weak entries do not retain otherwise unreferenced files.
    pub fn load(
        options: ProgramOptions,
        cache: &mut FileCache,
        counters: &Counters,
    ) -> Result<Self, tsr_compiler::Error> {
        Ok(Self::from_program(
            Arc::new(Program::load(options, cache, counters)?),
            counters,
        ))
    }

    /// Adopt an already loaded program without copying its graph. A fresh
    /// checker identity prevents results from another session from aliasing.
    pub fn from_program(program: Arc<Program>, counters: &Counters) -> Self {
        Self {
            program,
            identity: CheckerIdentity::new(Generation::new(counters), counters),
            counters: counters.clone(),
            checker: OnceLock::new(),
            emitter: OnceLock::new(),
        }
    }

    pub fn program(&self) -> &Arc<Program> {
        &self.program
    }

    pub fn checker(&self) -> Result<&Arc<CheckerOwner>, Error> {
        if let Some(result) = self.checker.get() {
            return result.as_ref().map_err(Clone::clone);
        }
        // Acquire before entering OnceLock: a host callback that reenters the
        // same session receives Reentry instead of waiting on its own cell.
        let _initialization = self.identity.lease()?;
        self.checker
            .get_or_init(|| {
                CheckerOwner::for_program(
                    self.identity.clone(),
                    &self.counters,
                    Arc::new(tsr_compiler::ProgramCheckerHost::new(self.program.clone())),
                )
                .map(Arc::new)
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// Queries borrow this scope; mutable checker storage never escapes it.
    pub fn operation(&self) -> Result<Operation<'_>, Error> {
        self.checker()?.operation()
    }

    /// Emit the program as `CheckedProgram::emit` does, with a write callback
    /// that keeps every output in memory: nothing is written through the
    /// program's host or to any file system. The emit uses the compiler's
    /// checker pool over this session's program (the pin's `Program.Emit`
    /// with the program's own pool), created on the first emit and kept for
    /// later ones, so its bytes are the compiler's whatever the query checker
    /// has done; its work groups follow the program's single-threaded setting
    /// and run on the caller where no thread can start (WebAssembly).
    ///
    /// The pin's `HandleNoEmitOptions` applies: with `noEmit` and no target
    /// files the result is empty and not skipped; with target files it is
    /// skipped. A target file of another program fails with `WrongOwner`.
    /// After [`Session::retire`] emits fail; an emit already running stops at
    /// its next checker gate. A native panic during the emit retires the
    /// session.
    ///
    /// No acceptance claim: Phase 7 grades this entry point.
    pub fn emit(&self, options: &EmitOptions<'_>) -> Result<EmitOutput, tsr_compiler::Error> {
        let generation = self.identity.generation();
        generation.validate().map_err(Error::from)?;
        let target_source_files = options
            .target_source_files
            .map(|files| self.target_source_files(files))
            .transpose()?;
        let checked = self
            .emitter
            .get_or_init(|| CheckedProgram::new(self.program.clone(), &self.counters, None));
        // A retirement that raced the pool's creation saw no pool to retire.
        generation.validate().map_err(Error::from)?;

        // Outputs by name, each name's writes in write order: the emitter
        // lists a name in `EmittedFiles` once per successful write.
        let written: Mutex<HashMap<Vec<u8>, VecDeque<Vec<u8>>>> = Mutex::new(HashMap::new());
        let write_file: &WriteFile<'_> = &|name, text, _: &mut WriteFileData| {
            written
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry(name.to_vec())
                .or_default()
                .push_back(text.to_vec());
            Ok(())
        };
        let result = {
            let _retire = RetireOnUnwind::new(generation);
            checked.emit(
                &CheckerRequest::default(),
                &tsr_compiler::EmitOptions {
                    target_source_files: target_source_files.as_deref(),
                    emit_only: options.emit_only,
                    force_emit: options.force_emit,
                    write_file: Some(write_file),
                },
            )?
        }
        .expect("a request without a cancellation token always emits");

        let mut written = written.into_inner().unwrap_or_else(PoisonError::into_inner);
        let files = result
            .emitted_files
            .into_iter()
            .map(|name| {
                let text = written
                    .get_mut(name.as_bytes())
                    .and_then(VecDeque::pop_front)
                    .expect("the in-memory callback never fails, so it wrote every emitted file");
                EmittedFile { name, text }
            })
            .collect();
        Ok(EmitOutput {
            emit_skipped: result.emit_skipped,
            diagnostics: result.diagnostics,
            files,
        })
    }

    /// The program's own handles of `files`, in their order.
    fn target_source_files(
        &self,
        files: &[&ProgramFile],
    ) -> Result<Vec<Arc<ProgramFile>>, tsr_compiler::Error> {
        let by_source: HashMap<NodeId, &Arc<ProgramFile>> = self
            .program
            .files()
            .iter()
            .map(|file| (file.source(), file))
            .collect();
        files
            .iter()
            .map(|file| {
                by_source
                    .get(&file.source())
                    .map(|&file| file.clone())
                    .ok_or_else(|| tsr_arena::Error::WrongOwner.into())
            })
            .collect()
    }

    /// Explicit cancellation is distinct from dropping a retention root.
    pub fn retire(&self) {
        self.identity.generation().retire();
        if let Some(pool) = self
            .emitter
            .get()
            .and_then(CheckedProgram::compiler_checker_pool)
        {
            pool.generation().retire();
        }
    }
}

/// Retires the session when an emit unwinds, as an unwinding query's lease
/// retires it; an emit called while another panic unwinds does not.
struct RetireOnUnwind<'a> {
    generation: &'a Generation,
    already_panicking: bool,
}

impl<'a> RetireOnUnwind<'a> {
    fn new(generation: &'a Generation) -> Self {
        Self {
            generation,
            already_panicking: std::thread::panicking(),
        }
    }
}

impl Drop for RetireOnUnwind<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() && !self.already_panicking {
            self.generation.retire();
        }
    }
}
