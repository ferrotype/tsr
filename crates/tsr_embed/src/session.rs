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
use tsr_compiler::{CheckedProgram, ProgramLike, WriteFile, WriteFileData};
pub use tsr_compiler::{EmitOnly, FileCache, Program, ProgramFile, ProgramOptions};
pub use tsr_compiler::{SourceMapEmitResult, WriteFileData as EmitWriteData};
use tsr_incremental::{BuildInfoReader, CompilerHost, ProgramCompilerHost};
use tsr_jsstring::JsString;

/// Cheap, explicitly retained root for a loaded program and its checker.
/// Query initialization occurs on the first query, under the checker's identity
/// permit. Reentry fails before waiting on the initialization cell. Failed
/// initialization is remembered; a panic retires the generation via the permit.
pub struct Session {
    program: Arc<Program>,
    identity: Arc<CheckerIdentity>,
    counters: Counters,
    checker: OnceLock<Result<Arc<CheckerOwner>, Error>>,
    /// Created by configured incremental adoption or the first compiler-pool
    /// diagnostic, query or emit operation.
    /// Its checkers are separate from the query checker above.
    emitter: OnceLock<SessionProgram>,
}

/// Services used while loading a configured program. The session retains the
/// mapper project; the build-info reader is borrowed only during construction.
/// The caller owns mapper project/host shutdown and may share them across loads.
#[derive(Default)]
pub struct SessionServices<'a> {
    pub content_mapper_project: Option<Arc<dyn tsr_contentmapper::Project>>,
    pub build_info_reader: Option<&'a dyn BuildInfoReader>,
}

enum SessionProgram {
    Compiler(CheckedProgram),
    Incremental(Box<tsr_incremental::Program>),
}

impl SessionProgram {
    fn program_like(&self) -> &dyn ProgramLike {
        match self {
            Self::Compiler(program) => program,
            Self::Incremental(program) => program.as_ref(),
        }
    }

    fn checked_program(&self) -> &CheckedProgram {
        self.program_like().checked_program()
    }
}

/// Read build info through this program's filesystem using the production
/// reader. A caller may decorate this reader before configured adoption.
pub fn build_info_reader(program: Arc<Program>) -> Box<dyn BuildInfoReader> {
    tsr_incremental::new_build_info_reader(Arc::new(ProgramCompilerHost::new(program)))
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
#[derive(Clone, Default)]
pub struct EmitOutput {
    /// `EmitResult.EmitSkipped`.
    pub emit_skipped: bool,
    /// `EmitResult.Diagnostics`: declaration emit diagnostics, and with
    /// `noEmitOnError` the program's diagnostics that skipped the emit.
    pub diagnostics: Vec<Diagnostic>,
    /// Files named by `EmitResult.EmittedFiles`, in that order. Unlisted write
    /// callbacks retain their files in `writes` instead.
    pub files: Vec<EmittedFile>,
    /// Successful in-memory write callbacks, in callback order. Unlike
    /// `files`, this order is not the compiler's combined file order.
    pub writes: Vec<EmitWrite>,
    /// The emitter's records, including its input-source lookup names;
    /// these are not reconstructed from output map files.
    pub source_maps: Vec<SourceMapEmitResult>,
    program: Option<Arc<Program>>,
}

impl EmitOutput {
    /// Retains the source owners of diagnostics and write metadata after the
    /// session drops. Only a manually default-constructed empty output has none.
    pub fn program(&self) -> Option<&Arc<Program>> {
        self.program.as_ref()
    }

    /// Resolve a callback record from this output without copying its bytes.
    /// Supply a record originating from this output: listed records resolve
    /// by index without checking ownership, so a foreign record may resolve
    /// another file or return `None`. Unlisted records carry their own file.
    pub fn written_file<'a>(&'a self, write: &'a EmitWrite) -> Option<&'a EmittedFile> {
        match write.emitted_file_index {
            Some(index) => self.files.get(index),
            None => write.unlisted_file(),
        }
    }
}

impl std::fmt::Debug for EmitOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmitOutput")
            .field("emit_skipped", &self.emit_skipped)
            .field("diagnostics", &self.diagnostics)
            .field("files", &self.files)
            .field("writes", &self.writes)
            .field("source_maps", &self.source_maps)
            .finish_non_exhaustive()
    }
}

/// A write callback's metadata and the location of its owned output bytes.
#[derive(Clone, Debug)]
pub struct EmitWrite {
    /// Index into `EmitOutput.files`. `None` means the compiler called the
    /// writer without listing this callback in `EmitResult.EmittedFiles`.
    pub emitted_file_index: Option<usize>,
    pub data: EmitWriteData,
    unlisted_file: Option<EmittedFile>,
}

impl EmitWrite {
    /// A callback not listed by the compiler still retains its name and bytes.
    pub fn unlisted_file(&self) -> Option<&EmittedFile> {
        self.unlisted_file.as_ref()
    }
}

/// Which production program diagnostic operation to execute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticKind {
    Config,
    Program,
    Syntactic,
    Bind,
    Semantic,
    Global,
    Declaration,
    Suggestion,
}

/// Every compiler checker held in checker order for one callback. File
/// queries use the same checker associations and state as diagnostics and emit.
/// Types cannot be mixed across checkers; retain them explicitly to escape.
pub struct CompilerOperations<'a> {
    operations: Vec<Operation<'a>>,
    checker_of: HashMap<NodeId, usize>,
}

impl<'a> CompilerOperations<'a> {
    /// Selects only a file belonging to this session, including with one checker.
    pub fn for_file(&mut self, file: &ProgramFile) -> Result<&mut Operation<'a>, Error> {
        let index = *self
            .checker_of
            .get(&file.source())
            .ok_or(tsr_arena::Error::WrongOwner)?;
        Ok(&mut self.operations[index])
    }

    pub fn len(&self) -> usize {
        self.operations.len()
    }

    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }

    /// Every held operation, in compiler checker order.
    pub fn iter(&self) -> impl Iterator<Item = &Operation<'a>> {
        self.operations.iter()
    }
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
    /// `incremental: true` uses the production incremental program and reads
    /// existing build info from the host; emission can include build-info output.
    pub fn load(
        options: ProgramOptions,
        cache: &mut FileCache,
        counters: &Counters,
    ) -> Result<Self, tsr_compiler::Error> {
        Self::load_with_services(options, &SessionServices::default(), cache, counters)
    }

    /// Load with a caller-owned mapper project and optional build-info reader.
    /// The reader is consulted only for incremental configuration. Its data is
    /// subject to the production version and mapper-identity validity checks.
    pub fn load_with_services(
        options: ProgramOptions,
        services: &SessionServices<'_>,
        cache: &mut FileCache,
        counters: &Counters,
    ) -> Result<Self, tsr_compiler::Error> {
        let program = Arc::new(Program::load_with_content_mapper_project(
            options,
            services.content_mapper_project.clone(),
            cache,
            counters,
        )?);
        Self::from_program_with_build_info_reader(program, services.build_info_reader, counters)
    }

    /// Adopt an already loaded program without copying its graph. A fresh
    /// checker identity prevents results from another session from aliasing.
    /// This compatibility entry point uses plain compilation, even when the
    /// configuration requests incremental mode. Use configured adoption below
    /// to honor that option and load previous build info.
    pub fn from_program(program: Arc<Program>, counters: &Counters) -> Self {
        Self {
            program,
            identity: CheckerIdentity::new(Generation::new(counters), counters),
            counters: counters.clone(),
            checker: OnceLock::new(),
            emitter: OnceLock::new(),
        }
    }

    /// Adopt a program and honor its incremental configuration. `None` reads
    /// build info from its filesystem. This can fail during incremental setup;
    /// the reader itself is not retained. Nonincremental adoption stays lazy.
    pub fn from_program_with_build_info_reader(
        program: Arc<Program>,
        reader: Option<&dyn BuildInfoReader>,
        counters: &Counters,
    ) -> Result<Self, tsr_compiler::Error> {
        let session = Self::from_program(program, counters);
        if session.program.options().incremental.is_true() {
            let host: Arc<dyn CompilerHost> =
                Arc::new(ProgramCompilerHost::new(session.program.clone()));
            let default_reader;
            let reader = if let Some(reader) = reader {
                reader
            } else {
                default_reader = tsr_incremental::new_build_info_reader(host.clone());
                default_reader.as_ref()
            };
            let old = tsr_incremental::read_build_info_program(
                session.program.config(),
                reader,
                host.as_ref(),
            );
            let checked = Arc::new(CheckedProgram::new(session.program.clone(), counters, None));
            let incremental = tsr_incremental::new_program(
                checked,
                old.as_ref(),
                tsr_incremental::create_host(host),
                None,
                false,
            )?;
            // The session is fresh and cannot have initialized another backend.
            assert!(session
                .emitter
                .set(SessionProgram::Incremental(Box::new(incremental)))
                .is_ok());
        }
        Ok(session)
    }

    pub fn program(&self) -> &Arc<Program> {
        &self.program
    }

    pub fn checker(&self) -> Result<&Arc<CheckerOwner>, Error> {
        self.validate()?;
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

    /// Execute a diagnostic phase using the compiler pool retained by emit.
    /// `None` selects every file for file phases; Config, Program and Global
    /// are program-wide. A supplied foreign file fails before any phase runs.
    /// Cancellation follows the selected production method. A canceled
    /// semantic check that reaches a checker poisons it for later checks; an
    /// incremental cancellation gate may return before touching that checker.
    /// The returned node identities borrow the session's source owners.
    pub fn diagnostics(
        &self,
        request: &CheckerRequest,
        kind: DiagnosticKind,
        file: Option<&ProgramFile>,
    ) -> Result<Vec<Diagnostic>, tsr_compiler::Error> {
        let _permit = self.identity.lease().map_err(Error::from)?;
        let _retire = RetireOnUnwind::new(self);
        if let Some(file) = file {
            self.target_source_files(&[file])?;
        }
        let checked = self.compiled()?.program_like();
        match kind {
            DiagnosticKind::Config => Ok(checked.config_file_parsing_diagnostics()),
            DiagnosticKind::Program => checked.program_diagnostics(),
            DiagnosticKind::Syntactic => checked.syntactic_diagnostics(request, file),
            DiagnosticKind::Bind => checked.bind_diagnostics(request, file),
            DiagnosticKind::Semantic => checked.semantic_diagnostics(request, file),
            DiagnosticKind::Global => checked.global_diagnostics(request),
            DiagnosticKind::Declaration => checked.declaration_diagnostics(request, file),
            DiagnosticKind::Suggestion => checked.suggestion_diagnostics(request, file),
        }
    }

    /// Holds all compiler checkers for `task`, so successive type and symbol
    /// walks observe emission's actual state. Reentry through this session
    /// fails before waiting; panic retires both its query and compiler pools.
    /// The compiler program stays private; operations retain their normal
    /// ownership and retirement gates.
    pub fn with_compiler_operations<R>(
        &self,
        task: impl FnOnce(&mut CompilerOperations<'_>) -> Result<R, Error>,
    ) -> Result<R, tsr_compiler::Error> {
        let _permit = self.identity.lease().map_err(Error::from)?;
        let _retire = RetireOnUnwind::new(self);
        let pool = self
            .checked()?
            .compiler_checker_pool()
            .expect("the session constructs the compiler's pool");
        let operations = pool
            .checkers()?
            .iter()
            .map(CheckerOwner::operation)
            .collect::<Result<Vec<_>, _>>()?;
        let checker_of = self
            .program
            .files()
            .iter()
            .zip(&pool.association_plan()?.associations)
            .map(|(file, &checker)| (file.source(), checker))
            .collect();
        Ok(task(&mut CompilerOperations {
            operations,
            checker_of,
        })?)
    }

    fn validate(&self) -> Result<(), Error> {
        let valid = self.identity.generation().validate().and_then(|()| {
            self.emitter
                .get()
                .and_then(|program| program.checked_program().compiler_checker_pool())
                .map_or(Ok(()), |pool| pool.generation().validate())
        });
        if valid.is_err() {
            self.retire();
        }
        Ok(valid?)
    }

    fn compiled(&self) -> Result<&SessionProgram, Error> {
        self.validate()?;
        let checked = self.emitter.get_or_init(|| {
            SessionProgram::Compiler(CheckedProgram::new(
                self.program.clone(),
                &self.counters,
                None,
            ))
        });
        // Retirement may have raced the pool's creation.
        self.validate()?;
        Ok(checked)
    }

    fn checked(&self) -> Result<&CheckedProgram, Error> {
        Ok(self.compiled()?.checked_program())
    }

    /// Emit through the configured production program, with a write callback
    /// that keeps every output in memory: nothing is written through the
    /// program's host or to any file system. The emit uses the compiler's
    /// checker pool over this session's program (the pin's `Program.Emit`
    /// with the program's own pool), created on the first emit and kept for
    /// later ones, so its bytes are the compiler's whatever the query checker
    /// has done; its work groups follow the program's single-threaded setting
    /// and run on the caller where no thread can start (WebAssembly).
    ///
    /// The pin's `HandleNoEmitOptions` applies: plain compilation with `noEmit`
    /// and no target files is empty and not skipped; target files are skipped.
    /// Incremental compilation may still write build info. A target file of
    /// another program fails with `WrongOwner`.
    /// After [`Session::retire`] emits fail; an emit already running stops at
    /// its next checker gate. A native panic during the emit retires the
    /// session.
    ///
    /// Incremental sessions apply the production incremental schedule and may
    /// write build info even with `noEmit`; `from_program` keeps plain semantics.
    /// No acceptance claim: Phase 7 grades this entry point.
    pub fn emit(&self, options: &EmitOptions<'_>) -> Result<EmitOutput, tsr_compiler::Error> {
        Ok(self
            .emit_with_request(&CheckerRequest::default(), options)?
            .expect("a request without a cancellation token always emits"))
    }

    /// The same in-memory emit with a caller's request. `None` is the pin's
    /// canceled result; `force_emit` follows the compiler's cancellation rules.
    pub fn emit_with_request(
        &self,
        request: &CheckerRequest,
        options: &EmitOptions<'_>,
    ) -> Result<Option<EmitOutput>, tsr_compiler::Error> {
        let _permit = self.identity.lease().map_err(Error::from)?;
        let _retire = RetireOnUnwind::new(self);
        let target_source_files = options
            .target_source_files
            .map(|files| self.target_source_files(files))
            .transpose()?;
        let checked = self.compiled()?.program_like();

        // Outputs by name, each name's writes in write order: the emitter
        // lists a name in `EmittedFiles` once per successful write.
        let written: Mutex<Vec<(EmittedFile, WriteFileData)>> = Mutex::new(Vec::new());
        let write_file: &WriteFile<'_> = &|name, text, data: &mut WriteFileData| {
            written
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((
                    EmittedFile {
                        name: JsString::from_bytes(name),
                        text: text.to_vec(),
                    },
                    data.clone(),
                ));
            Ok(())
        };
        let result = {
            checked.emit(
                request,
                &tsr_compiler::EmitOptions {
                    target_source_files: target_source_files.as_deref(),
                    emit_only: options.emit_only,
                    force_emit: options.force_emit,
                    write_file: Some(write_file),
                },
            )?
        };
        let Some(result) = result else {
            return Ok(None);
        };

        let mut written: Vec<_> = written
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner)
            .into_iter()
            .map(|(file, data)| (Some(file), data))
            .collect();
        let mut by_name: HashMap<Vec<u8>, VecDeque<usize>> = HashMap::new();
        for (index, (file, _)) in written.iter().enumerate() {
            by_name
                .entry(
                    file.as_ref()
                        .expect("each callback starts with its file")
                        .name
                        .as_bytes()
                        .to_vec(),
                )
                .or_default()
                .push_back(index);
        }
        let mut emitted_indices = vec![None; written.len()];
        let files = result
            .emitted_files
            .into_iter()
            .enumerate()
            .map(|(emitted_index, name)| {
                let write_index = by_name
                    .get_mut(name.as_bytes())
                    .and_then(VecDeque::pop_front)
                    .expect("the in-memory callback never fails, so it wrote every emitted file");
                emitted_indices[write_index] = Some(emitted_index);
                let file = written[write_index]
                    .0
                    .take()
                    .expect("a callback's bytes enter emitted files exactly once");
                EmittedFile {
                    name,
                    text: file.text,
                }
            })
            .collect();
        let writes = written
            .into_iter()
            .zip(emitted_indices)
            .map(|((unlisted_file, data), emitted_file_index)| EmitWrite {
                emitted_file_index,
                data,
                unlisted_file,
            })
            .collect();
        Ok(Some(EmitOutput {
            emit_skipped: result.emit_skipped,
            diagnostics: result.diagnostics,
            files,
            writes,
            source_maps: result.source_maps,
            program: Some(self.program.clone()),
        }))
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
            .and_then(|program| program.checked_program().compiler_checker_pool())
        {
            pool.generation().retire();
        }
    }
}

/// Retires the session when an emit unwinds, as an unwinding query's lease
/// retires it; an emit called while another panic unwinds does not.
struct RetireOnUnwind<'a> {
    session: &'a Session,
    already_panicking: bool,
}

impl<'a> RetireOnUnwind<'a> {
    fn new(session: &'a Session) -> Self {
        Self {
            session,
            already_panicking: std::thread::panicking(),
        }
    }
}

impl Drop for RetireOnUnwind<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() && !self.already_panicking {
            self.session.retire();
        }
    }
}
