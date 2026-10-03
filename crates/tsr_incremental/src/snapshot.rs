//! The incremental state (`snapshot.go`): each file's version and
//! signature, its references, its cached diagnostics, what is pending emit,
//! and the hashes that identify a file's text and its declaration output.
//!
//! The pin's `SyncMap`s and `SyncSet`s are maps behind a mutex here; their
//! iteration order is the pin's random order wherever the pin sorts or the
//! result does not depend on it, and path order otherwise.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use tsr_ast::diagnostic_api::{
    RepopulateDiagnosticInfo, REPOPULATE_MODE_MISMATCH, REPOPULATE_MODULE_NOT_FOUND,
};
use tsr_ast::{Diagnostic, NodeId};
use tsr_compiler::{Error, Program, ProgramFile, WriteFileData};
use tsr_core::{CompilerOptions, ModuleKind, TextRange, Tristate};
use tsr_jsstring::JsString;

/// `tspath.Path`: a file's canonical path.
pub type Path = JsString;

/// The value under a mutex, poisoned or not: every write this crate makes
/// under one of its locks is a single insert, removal or assignment.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `FileInfo`: a file's version (the hash of its text), its signature (the
/// hash of its declaration output, or its version), whether it affects the
/// global scope and its implied module format.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileInfo {
    pub(crate) version: JsString,
    pub(crate) signature: JsString,
    pub(crate) affects_global_scope: bool,
    pub(crate) implied_node_format: ModuleKind,
}

impl FileInfo {
    // port: tsc/internal/execute/incremental/snapshot.go:FileInfo.Version
    pub fn version(&self) -> &JsString {
        &self.version
    }
    // port: tsc/internal/execute/incremental/snapshot.go:FileInfo.Signature
    pub fn signature(&self) -> &JsString {
        &self.signature
    }
    // port: tsc/internal/execute/incremental/snapshot.go:FileInfo.AffectsGlobalScope
    pub fn affects_global_scope(&self) -> bool {
        self.affects_global_scope
    }
    // port: tsc/internal/execute/incremental/snapshot.go:FileInfo.ImpliedNodeFormat
    pub fn implied_node_format(&self) -> ModuleKind {
        self.implied_node_format
    }
}

/// The lowercase hex of the text's 128-bit xxh3 digest (big-endian, as the
/// pinned `xxh3.Uint128.Bytes`), followed by `-` and the text itself when
/// `hash_with_text` (testing).
// port: tsc/internal/execute/incremental/snapshot.go:ComputeHash
pub fn compute_hash(text: &[u8], hash_with_text: bool) -> JsString {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let hash_bytes = xxhash_rust::xxh3::xxh3_128(text).to_be_bytes();
    let digits = hash_bytes.len() * 2;
    let mut hash = Vec::with_capacity(if hash_with_text {
        digits + 1 + text.len()
    } else {
        digits
    });
    for byte in hash_bytes {
        hash.extend_from_slice(&[HEX[usize::from(byte >> 4)], HEX[usize::from(byte & 0xf)]]);
    }
    if hash_with_text {
        hash.push(b'-');
        hash.extend_from_slice(text);
    }
    JsString::from_bytes(hash)
}

/// `FileEmitKind`: which outputs of a file are pending.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FileEmitKind(pub u32);

impl FileEmitKind {
    pub const NONE: Self = Self(0);
    /// emit js file
    pub const JS: Self = Self(1 << 0);
    /// emit js.map file
    pub const JS_MAP: Self = Self(1 << 1);
    /// emit inline source map in js file
    pub const JS_INLINE_MAP: Self = Self(1 << 2);
    /// emit dts errors
    pub const DTS_ERRORS: Self = Self(1 << 3);
    /// emit d.ts file
    pub const DTS_EMIT: Self = Self(1 << 4);
    /// emit d.ts.map file
    pub const DTS_MAP: Self = Self(1 << 5);

    pub const DTS: Self = Self(Self::DTS_ERRORS.0 | Self::DTS_EMIT.0);
    pub const ALL_JS: Self = Self(Self::JS.0 | Self::JS_MAP.0 | Self::JS_INLINE_MAP.0);
    pub const ALL_DTS_EMIT: Self = Self(Self::DTS_EMIT.0 | Self::DTS_MAP.0);
    pub const ALL_DTS: Self = Self(Self::DTS.0 | Self::DTS_MAP.0);
    pub const ALL: Self = Self(Self::ALL_JS.0 | Self::ALL_DTS.0);
}

impl std::ops::BitOr for FileEmitKind {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}
impl std::ops::BitOrAssign for FileEmitKind {
    fn bitor_assign(&mut self, other: Self) {
        self.0 |= other.0;
    }
}
impl std::ops::BitAnd for FileEmitKind {
    type Output = Self;
    fn bitand(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}
impl std::ops::BitAndAssign for FileEmitKind {
    fn bitand_assign(&mut self, other: Self) {
        self.0 &= other.0;
    }
}
impl std::ops::BitXor for FileEmitKind {
    type Output = Self;
    fn bitxor(self, other: Self) -> Self {
        Self(self.0 ^ other.0)
    }
}

// port: tsc/internal/execute/incremental/snapshot.go:GetFileEmitKind
pub fn get_file_emit_kind(options: &CompilerOptions) -> FileEmitKind {
    let mut result = FileEmitKind::JS;
    if options.source_map.is_true() {
        result |= FileEmitKind::JS_MAP;
    }
    if options.inline_source_map.is_true() {
        result |= FileEmitKind::JS_INLINE_MAP;
    }
    if options.emit_declarations() {
        result |= FileEmitKind::DTS;
    }
    if options.declaration_map.is_true() {
        result |= FileEmitKind::DTS_MAP;
    }
    if options.emit_declaration_only.is_true() {
        result &= FileEmitKind::ALL_DTS;
    }
    result
}

// port: tsc/internal/execute/incremental/snapshot.go:getPendingEmitKindWithOptions
pub(crate) fn get_pending_emit_kind_with_options(
    options: &CompilerOptions,
    old_options: &CompilerOptions,
) -> FileEmitKind {
    let old_emit_kind = get_file_emit_kind(old_options);
    let new_emit_kind = get_file_emit_kind(options);
    get_pending_emit_kind(new_emit_kind, old_emit_kind)
}

// port: tsc/internal/execute/incremental/snapshot.go:getPendingEmitKind
pub(crate) fn get_pending_emit_kind(
    emit_kind: FileEmitKind,
    old_emit_kind: FileEmitKind,
) -> FileEmitKind {
    if old_emit_kind == emit_kind {
        return FileEmitKind::NONE;
    }
    if old_emit_kind == FileEmitKind::NONE || emit_kind == FileEmitKind::NONE {
        return emit_kind;
    }
    let diff = old_emit_kind ^ emit_kind;
    let mut result = FileEmitKind::NONE;
    // If there is diff in Js emit, pending emit is js emit flags
    if (diff & FileEmitKind::ALL_JS) != FileEmitKind::NONE {
        result |= emit_kind & FileEmitKind::ALL_JS;
    }
    // If dts errors pending, add dts errors flag
    if (diff & FileEmitKind::DTS_ERRORS) != FileEmitKind::NONE {
        result |= emit_kind & FileEmitKind::ALL_DTS;
    }
    // If there is diff in Dts emit, pending emit is dts emit flags
    if (diff & FileEmitKind::ALL_DTS_EMIT) != FileEmitKind::NONE {
        result |= emit_kind & FileEmitKind::ALL_DTS_EMIT;
    }
    result
}

/// Signature (Hash of d.ts emitted), is string if it was emitted using same
/// d.ts.map option as what compilerOptions indicate, otherwise tuple of
/// string.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EmitSignature {
    pub(crate) signature: JsString,
    pub(crate) signature_with_different_options: Option<Vec<JsString>>,
}

impl EmitSignature {
    /// Covert to Emit signature based on oldOptions and EmitSignature format
    /// If d.ts map options differ then swap the format, otherwise use as is
    // port: tsc/internal/execute/incremental/snapshot.go:emitSignature.getNewEmitSignature
    pub(crate) fn get_new_emit_signature(
        &self,
        old_options: &CompilerOptions,
        new_options: &CompilerOptions,
    ) -> Self {
        if old_options.declaration_map.is_true() == new_options.declaration_map.is_true() {
            return self.clone();
        }
        match &self.signature_with_different_options {
            None => Self {
                signature: JsString::default(),
                signature_with_different_options: Some(vec![self.signature.clone()]),
            },
            Some(signature_with_different_options) => Self {
                signature: signature_with_different_options[0].clone(),
                signature_with_different_options: None,
            },
        }
    }
}

/// `buildInfoDiagnosticWithFileName`: a build info's diagnostic with its
/// file as a path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildInfoDiagnosticWithFileName {
    /// filename if it is for a File thats other than its stored for
    pub(crate) file: Path,
    pub(crate) no_file: bool,
    pub(crate) pos: i64,
    pub(crate) end: i64,
    pub(crate) code: i32,
    pub(crate) category: i32,
    pub(crate) source: JsString,
    pub(crate) message_text: JsString,
    pub(crate) message_key: JsString,
    pub(crate) message_args: Option<Vec<JsString>>,
    pub(crate) message_chain: Option<Vec<BuildInfoDiagnosticWithFileName>>,
    pub(crate) related_information: Option<Vec<BuildInfoDiagnosticWithFileName>>,
    pub(crate) reports_unnecessary: bool,
    pub(crate) reports_deprecated: bool,
    pub(crate) skipped_on_no_emit: bool,
    pub(crate) repopulate_info: Option<Arc<RepopulateDiagnosticInfo>>,
}

/// Cached diagnostics retain exactly their referenced source owners, as Go's
/// diagnostic file pointers do. Retaining a complete Program would keep every
/// obsolete source and resolver alive across watch generations.
#[derive(Clone)]
pub struct ProgramDiagnostics {
    sources: BTreeMap<NodeId, DiagnosticSource>,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

#[derive(Clone)]
enum DiagnosticSource {
    Source(Arc<ProgramFile>),
    Config(tsr_tsoptions::TsConfigSourceFile),
}

impl ProgramDiagnostics {
    pub(crate) fn file_path(&self, id: NodeId) -> Result<Path, Error> {
        match self.sources.get(&id) {
            Some(DiagnosticSource::Source(source)) => crate::program::source_path(source),
            Some(DiagnosticSource::Config(source)) => Ok(source
                .file
                .view()
                .source_file(source.root)?
                .parse_options()
                .path
                .clone()),
            None => Err(tsr_arena::Error::WrongOwner.into()),
        }
    }

    fn new(program: &Program, diagnostics: Vec<Diagnostic>) -> Self {
        fn retain(
            diagnostic: &Diagnostic,
            program: &Program,
            sources: &mut BTreeMap<NodeId, DiagnosticSource>,
        ) {
            if let Some(id) = diagnostic.file {
                if let Some(file) = program.file_of_node(id) {
                    sources
                        .entry(id)
                        .or_insert_with(|| DiagnosticSource::Source(file.clone()));
                } else if let Some(file) = program.config_source(id) {
                    sources
                        .entry(id)
                        .or_insert_with(|| DiagnosticSource::Config(file.clone()));
                }
            }
            for nested in diagnostic
                .message_chain
                .iter()
                .chain(&diagnostic.related_information)
            {
                retain(nested, program, sources);
            }
        }
        let mut sources = BTreeMap::new();
        for diagnostic in &diagnostics {
            retain(diagnostic, program, &mut sources);
        }
        Self {
            sources,
            diagnostics,
        }
    }

    /// Rebind source identities when a caller reparses unchanged files instead
    /// of using FileCache. The shared cached entry remains immutable, and no
    /// whole-program owner is needed to resolve its original source paths.
    pub(crate) fn diagnostics_for(&self, program: &Arc<Program>) -> Result<Vec<Diagnostic>, Error> {
        self.rebind_list(&self.diagnostics, program)
    }

    pub(crate) fn rebind_list(
        &self,
        diagnostics: &[Diagnostic],
        program: &Arc<Program>,
    ) -> Result<Vec<Diagnostic>, Error> {
        diagnostics
            .iter()
            .map(|d| rebind_diagnostic(d, &self.sources, program))
            .collect()
    }
}

fn rebind_diagnostic(
    diagnostic: &Diagnostic,
    from: &BTreeMap<NodeId, DiagnosticSource>,
    to: &Program,
) -> Result<Diagnostic, Error> {
    let rebind_all = |diagnostics: &[Arc<Diagnostic>]| {
        diagnostics
            .iter()
            .map(|diagnostic| rebind_diagnostic(diagnostic, from, to).map(Arc::new))
            .collect::<Result<Vec<_>, Error>>()
    };
    let mut rebound = diagnostic.clone();
    rebound.file = diagnostic
        .file
        .map(|file| rebind_file(file, from, to))
        .transpose()?;
    rebound.message_chain = rebind_all(&diagnostic.message_chain)?;
    rebound.related_information = rebind_all(&diagnostic.related_information)?;
    Ok(rebound)
}

fn rebind_file(
    file: NodeId,
    from: &BTreeMap<NodeId, DiagnosticSource>,
    to: &Program,
) -> Result<NodeId, Error> {
    if to.file_of_node(file).is_some() || to.config_source(file).is_some() {
        return Ok(file);
    }
    match from.get(&file) {
        Some(DiagnosticSource::Source(source)) => {
            let path = crate::program::source_path(source)?;
            Ok(to.file(path.as_bytes()).map_or(file, ProgramFile::source))
        }
        Some(DiagnosticSource::Config(source)) => {
            // Config diagnostics are invalidated by their option/config change;
            // until then their original file remains a retained capability.
            source.file.view().node(file)?;
            Ok(file)
        }
        None => Err(tsr_arena::Error::WrongOwner.into()),
    }
}

impl std::fmt::Debug for ProgramDiagnostics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgramDiagnostics")
            .field("diagnostics", &self.diagnostics)
            .finish_non_exhaustive()
    }
}

/// `DiagnosticsOrBuildInfoDiagnosticsWithFileName`: a file's cached
/// diagnostics, either as a program reported them or as a build info
/// recorded them (converted on first read).
#[derive(Debug, Default)]
pub struct DiagnosticsOrBuildInfoDiagnosticsWithFileName {
    pub(crate) diagnostics: Mutex<Option<ProgramDiagnostics>>,
    pub(crate) build_info_diagnostics: Vec<BuildInfoDiagnosticWithFileName>,
}

impl DiagnosticsOrBuildInfoDiagnosticsWithFileName {
    pub(crate) fn from_diagnostics(program: &Arc<Program>, diagnostics: Vec<Diagnostic>) -> Self {
        Self {
            diagnostics: Mutex::new(Some(ProgramDiagnostics::new(program, diagnostics))),
            build_info_diagnostics: Vec::new(),
        }
    }

    /// The program diagnostics, if any (the pin's `d.diagnostics`).
    pub(crate) fn program_diagnostics(&self) -> Option<ProgramDiagnostics> {
        lock(&self.diagnostics).clone()
    }

    pub(crate) fn has_diagnostics(&self) -> bool {
        lock(&self.diagnostics)
            .as_ref()
            .is_some_and(|diagnostics| !diagnostics.diagnostics.is_empty())
            || !self.build_info_diagnostics.is_empty()
    }

    // port: tsc/internal/execute/incremental/snapshot.go:DiagnosticsOrBuildInfoDiagnosticsWithFileName.getDiagnostics
    pub(crate) fn get_diagnostics(
        &self,
        p: &Arc<Program>,
        file: Option<&ProgramFile>,
    ) -> Result<Vec<Diagnostic>, Error> {
        let mut diagnostics = lock(&self.diagnostics);
        if let Some(diagnostics) = diagnostics.as_ref() {
            return diagnostics.diagnostics_for(p);
        }
        // Convert and cache the diagnostics
        let mut converted = Vec::with_capacity(self.build_info_diagnostics.len());
        for diag in &self.build_info_diagnostics {
            converted.push(diag.to_diagnostic(p, file.map(ProgramFile::source))?);
        }
        if !self.build_info_diagnostics.is_empty() {
            *diagnostics = Some(ProgramDiagnostics::new(p, converted.clone()));
        }
        Ok(converted)
    }
}

impl BuildInfoDiagnosticWithFileName {
    // port: tsc/internal/execute/incremental/snapshot.go:buildInfoDiagnosticWithFileName.toDiagnostic
    pub(crate) fn to_diagnostic(
        &self,
        p: &Arc<Program>,
        file: Option<NodeId>,
    ) -> Result<Diagnostic, Error> {
        let file_for_diagnostic = if !self.file.is_empty() {
            p.file(self.file.as_bytes()).map(ProgramFile::source)
        } else if !self.no_file {
            file
        } else {
            None
        };

        if self.repopulate_info.is_some() {
            return repopulate_diagnostic_chain(self, p, file_for_diagnostic);
        }

        let mut message_chain =
            Vec::with_capacity(self.message_chain.as_deref().unwrap_or_default().len());
        for msg in self.message_chain.iter().flatten() {
            message_chain.push(Arc::new(msg.to_diagnostic(p, file_for_diagnostic)?));
        }
        let mut related_information = Vec::with_capacity(
            self.related_information
                .as_deref()
                .unwrap_or_default()
                .len(),
        );
        for info in self.related_information.iter().flatten() {
            related_information.push(Arc::new(info.to_diagnostic(p, file_for_diagnostic)?));
        }
        let mut diagnostic = Diagnostic::from_serialized(
            file_for_diagnostic,
            TextRange::new(self.pos, self.end),
            self.code,
            self.category,
            self.message_key.clone(),
            self.message_args.clone().unwrap_or_default(),
            message_chain,
            related_information,
            self.reports_unnecessary,
            self.reports_deprecated,
            self.skipped_on_no_emit,
        );
        if !self.source.is_empty() || !self.message_text.is_empty() {
            diagnostic.source.clone_from(&self.source);
            diagnostic.message_text.clone_from(&self.message_text);
        }
        Ok(diagnostic)
    }

    // port: tsc/internal/execute/incremental/snapshot.go:buildInfoDiagnosticWithFileName.toDiagnosticWithoutRepopulate
    fn to_diagnostic_without_repopulate(
        &self,
        p: &Arc<Program>,
        file: Option<NodeId>,
    ) -> Result<Diagnostic, Error> {
        let mut message_chain =
            Vec::with_capacity(self.message_chain.as_deref().unwrap_or_default().len());
        for msg in self.message_chain.iter().flatten() {
            message_chain.push(Arc::new(msg.to_diagnostic(p, file)?));
        }
        let mut related_information = Vec::with_capacity(
            self.related_information
                .as_deref()
                .unwrap_or_default()
                .len(),
        );
        for info in self.related_information.iter().flatten() {
            related_information.push(Arc::new(info.to_diagnostic(p, file)?));
        }
        Ok(Diagnostic::from_serialized(
            file,
            TextRange::new(self.pos, self.end),
            self.code,
            self.category,
            self.message_key.clone(),
            self.message_args.clone().unwrap_or_default(),
            message_chain,
            related_information,
            self.reports_unnecessary,
            self.reports_deprecated,
            self.skipped_on_no_emit,
        ))
    }
}

/// repopulateDiagnosticChain recomputes a diagnostic chain entry that
/// depends on program state which may have changed between incremental
/// builds.
// port: tsc/internal/execute/incremental/snapshot.go:repopulateDiagnosticChain
pub(crate) fn repopulate_diagnostic_chain(
    b: &BuildInfoDiagnosticWithFileName,
    p: &Arc<Program>,
    file: Option<NodeId>,
) -> Result<Diagnostic, Error> {
    let info = b
        .repopulate_info
        .as_ref()
        .expect("a repopulated diagnostic has its repopulate info");
    match info.kind {
        REPOPULATE_MODE_MISMATCH => repopulate_mode_mismatch_chain(b, p, file),
        REPOPULATE_MODULE_NOT_FOUND => repopulate_module_not_found_chain(b, p, file, info),
        // Fall back to using the stored (possibly stale) data
        _ => b.to_diagnostic_without_repopulate(p, file),
    }
}

/// The name and path of the program file `file`.
fn file_names(p: &Program, file: NodeId) -> Result<(JsString, JsString), Error> {
    let program_file = p.file_of_node(file).ok_or(tsr_arena::Error::WrongOwner)?;
    let source = program_file.bound().view().source_file()?;
    Ok((
        source.parse_options().file_name.clone(),
        source.parse_options().path.clone(),
    ))
}

// port: tsc/internal/execute/incremental/snapshot.go:repopulateModeMismatchChain
fn repopulate_mode_mismatch_chain(
    b: &BuildInfoDiagnosticWithFileName,
    p: &Arc<Program>,
    file: Option<NodeId>,
) -> Result<Diagnostic, Error> {
    let Some(file) = file else {
        return b.to_diagnostic_without_repopulate(p, None);
    };

    let (file_name, path) = file_names(p, file)?;
    let host = crate::program::checker_host(p);
    let details =
        tsr_checker::create_mode_mismatch_details(&host, file_name.as_bytes(), path.as_bytes())?;

    let mut next_chain = Vec::with_capacity(b.message_chain.as_deref().unwrap_or_default().len());
    for msg in b.message_chain.iter().flatten() {
        next_chain.push(Arc::new(msg.to_diagnostic(p, Some(file))?));
    }

    Ok(Diagnostic::from_serialized(
        Some(file),
        TextRange::new(b.pos, b.end),
        details.message.code,
        details.message.category as i32,
        JsString::from_bytes(details.message.key.as_bytes()),
        details.args,
        next_chain,
        Vec::new(),
        false,
        false,
        false,
    ))
}

// port: tsc/internal/execute/incremental/snapshot.go:repopulateModuleNotFoundChain
fn repopulate_module_not_found_chain(
    b: &BuildInfoDiagnosticWithFileName,
    p: &Arc<Program>,
    file: Option<NodeId>,
    info: &RepopulateDiagnosticInfo,
) -> Result<Diagnostic, Error> {
    let Some(file) = file else {
        return b.to_diagnostic_without_repopulate(p, None);
    };

    let mut package_name = info.package_name.clone();
    if package_name.is_empty() {
        package_name.clone_from(&info.module_reference);
    }

    let (file_name, _) = file_names(p, file)?;
    let host = crate::program::checker_host(p);
    let details = tsr_checker::create_module_not_found_chain(
        &host,
        file_name.as_bytes(),
        &info.module_reference,
        ModuleKind(info.mode),
        package_name.as_bytes(),
    )?;

    let mut next_chain = Vec::with_capacity(b.message_chain.as_deref().unwrap_or_default().len());
    for msg in b.message_chain.iter().flatten() {
        next_chain.push(Arc::new(msg.to_diagnostic(p, Some(file))?));
    }

    Ok(Diagnostic::from_serialized(
        Some(file),
        TextRange::new(b.pos, b.end),
        details.message.code,
        details.message.category as i32,
        JsString::from_bytes(details.message.key.as_bytes()),
        details.args,
        next_chain,
        Vec::new(),
        false,
        false,
        false,
    ))
}

/// The incremental state of a program.
#[derive(Default)]
pub struct Snapshot {
    // These are the fields that get serialized
    /// Information of the file eg. its version, signature etc
    pub(crate) file_infos: Mutex<BTreeMap<Path, FileInfo>>,
    pub(crate) options: Arc<CompilerOptions>,
    /// Contains the map of ReferencedSet=Referenced files of the file if module emit is enabled
    pub(crate) referenced_map: crate::reference_map::ReferenceMap,
    /// Cache of semantic diagnostics for files with their Path being the key
    pub(crate) semantic_diagnostics_per_file:
        Mutex<BTreeMap<Path, Arc<DiagnosticsOrBuildInfoDiagnosticsWithFileName>>>,
    /// Cache of dts emit diagnostics for files with their Path being the key
    pub(crate) emit_diagnostics_per_file:
        Mutex<BTreeMap<Path, Arc<DiagnosticsOrBuildInfoDiagnosticsWithFileName>>>,
    /// The map has key by source file's path that has been changed
    pub(crate) changed_files_set: Mutex<BTreeSet<Path>>,
    /// Files pending to be emitted
    pub(crate) affected_files_pending_emit: Mutex<BTreeMap<Path, FileEmitKind>>,
    /// Hash of d.ts emitted for the file, use to track when emit of d.ts changes
    pub(crate) emit_signatures: Mutex<std::collections::HashMap<Path, EmitSignature>>,
    /// The fields the pin's snapshot mutates without a lock.
    pub(crate) state: Mutex<SnapshotState>,

    // Additional fields that are not serialized but needed to track state
    /// true if build info emit is pending
    pub(crate) build_info_emit_pending: AtomicBool,
    ///  Cache of all files excluding default library file for the current program
    pub(crate) all_files_excluding_default_library_file: OnceLock<Vec<Arc<ProgramFile>>>,

    /// Used with testing to add text of hash for better comparison
    pub(crate) hash_with_text: bool,
}

/// The snapshot's fields its owner reads and writes outside the pin's
/// synchronized maps.
#[derive(Clone, Debug, Default)]
pub struct SnapshotState {
    /// Name of the file whose dts was the latest to change
    pub(crate) latest_changed_dts_file: JsString,
    /// Recorded if program had errors that need to be reported even with --noCheck
    pub(crate) has_errors: Tristate,
    /// Recorded if program had semantic errors only for non incremental build
    pub(crate) has_semantic_errors: bool,
    /// If semantic diagnostic check is pending
    pub(crate) check_pending: bool,
    /// Looked up package.json files from
    pub(crate) package_jsons: Option<Vec<JsString>>,
    pub(crate) missing_package_jsons: Option<Vec<JsString>>,

    pub(crate) has_errors_from_old_state: Tristate,
    pub(crate) has_semantic_errors_from_old_state: bool,
    pub(crate) package_jsons_from_old_state: Option<Vec<JsString>>,
    pub(crate) missing_package_jsons_from_old_state: Option<Vec<JsString>>,
    pub(crate) has_changed_dts_file: bool,
    pub(crate) has_emit_diagnostics: bool,
}

impl std::fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Snapshot")
            .field("file_infos", &self.file_infos)
            .field("changed_files_set", &self.changed_files_set)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

/// Retained identity of a cached diagnostics entry. Holding this value keeps
/// its allocation alive, so replacement cannot reuse an address and compare
/// equal. The entry contents and cache mutation remain private.
#[derive(Clone, Debug)]
pub struct CachedDiagnosticsIdentity(Arc<DiagnosticsOrBuildInfoDiagnosticsWithFileName>);

impl PartialEq for CachedDiagnosticsIdentity {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for CachedDiagnosticsIdentity {}

impl Snapshot {
    /// Observe the same identity that Go's testing hook reads from
    /// `SemanticDiagnosticsPerFile.Load`, without exposing the mutable map.
    pub fn cached_semantic_diagnostics_identity(
        &self,
        path: &Path,
    ) -> Option<CachedDiagnosticsIdentity> {
        lock(&self.semantic_diagnostics_per_file)
            .get(path)
            .cloned()
            .map(CachedDiagnosticsIdentity)
    }

    // port: tsc/internal/execute/incremental/snapshot.go:snapshot.addFileToChangeSet
    pub(crate) fn add_file_to_change_set(&self, file_path: &Path) {
        lock(&self.changed_files_set).insert(file_path.clone());
        self.build_info_emit_pending.store(true, Ordering::SeqCst);
    }

    // port: tsc/internal/execute/incremental/snapshot.go:snapshot.addFileToAffectedFilesPendingEmit
    pub(crate) fn add_file_to_affected_files_pending_emit(
        &self,
        file_path: &Path,
        emit_kind: FileEmitKind,
    ) {
        {
            let mut pending = lock(&self.affected_files_pending_emit);
            let existing_kind = pending.get(file_path).copied().unwrap_or_default();
            pending.insert(file_path.clone(), existing_kind | emit_kind);
        }
        if emit_kind & FileEmitKind::DTS_ERRORS != FileEmitKind::NONE {
            lock(&self.emit_diagnostics_per_file).remove(file_path);
        }
        self.build_info_emit_pending.store(true, Ordering::SeqCst);
    }

    // port: tsc/internal/execute/incremental/snapshot.go:snapshot.getAllFilesExcludingDefaultLibraryFile
    pub(crate) fn get_all_files_excluding_default_library_file(
        &self,
        program: &Program,
        first_source_file: Option<&Arc<ProgramFile>>,
    ) -> Result<&[Arc<ProgramFile>], Error> {
        if let Some(files) = self.all_files_excluding_default_library_file.get() {
            return Ok(files);
        }
        let files = program.files();
        let mut all_files = Vec::with_capacity(files.len());
        let path_of = |file: &ProgramFile| -> Result<JsString, Error> {
            Ok(file
                .bound()
                .view()
                .source_file()?
                .parse_options()
                .path
                .clone())
        };
        let mut add_source_file = |file: &Arc<ProgramFile>| -> Result<(), Error> {
            if !program.is_lib(path_of(file)?.as_bytes()) {
                all_files.push(file.clone());
            }
            Ok(())
        };
        if let Some(first_source_file) = first_source_file {
            add_source_file(first_source_file)?;
        }
        for file in files {
            if first_source_file.is_none_or(|first| !Arc::ptr_eq(first, file)) {
                add_source_file(file)?;
            }
        }
        Ok(self
            .all_files_excluding_default_library_file
            .get_or_init(|| all_files))
    }

    // port: tsc/internal/execute/incremental/snapshot.go:snapshot.computeSignatureWithDiagnostics
    pub(crate) fn compute_signature_with_diagnostics(
        &self,
        program: &Program,
        file: NodeId,
        text: &[u8],
        data: &WriteFileData,
    ) -> Result<JsString, Error> {
        let mut builder = Vec::new();
        builder.extend_from_slice(get_text_handling_source_map_for_signature(text, data));
        for diag in &data.diagnostics {
            diagnostic_to_string_builder(program, Some(diag), file, &mut builder)?;
        }
        Ok(self.compute_hash(&builder))
    }

    // port: tsc/internal/execute/incremental/snapshot.go:snapshot.computeHash
    pub(crate) fn compute_hash(&self, text: &[u8]) -> JsString {
        compute_hash(text, self.hash_with_text)
    }

    // port: tsc/internal/execute/incremental/snapshot.go:snapshot.canUseIncrementalState
    pub(crate) fn can_use_incremental_state(&self) -> bool {
        if !self.options.is_incremental() && self.options.build.is_true() {
            // If not incremental build (with tsc -b), we don't need to track state except diagnostics per file so we can use it
            return false;
        }
        true
    }

    pub(crate) fn state(&self) -> MutexGuard<'_, SnapshotState> {
        lock(&self.state)
    }
}

// port: tsc/internal/execute/incremental/snapshot.go:getTextHandlingSourceMapForSignature
pub(crate) fn get_text_handling_source_map_for_signature<'a>(
    text: &'a [u8],
    data: &WriteFileData,
) -> &'a [u8] {
    if data.source_map_url_pos != -1 {
        return &text[..data.source_map_url_pos as usize];
    }
    text
}

/// The canonical path of the file `file` names, a program file or a
/// configuration source.
pub(crate) fn file_path(program: &Program, file: NodeId) -> Result<Path, Error> {
    if let Some(program_file) = program.file_of_node(file) {
        let source = program_file.bound().view().source_file()?;
        return Ok(source.parse_options().path.clone());
    }
    let config = program
        .config_source(file)
        .ok_or(tsr_arena::Error::WrongOwner)?;
    let source = config.file.view().source_file(config.root)?;
    Ok(source.parse_options().path.clone())
}

// port: tsc/internal/execute/incremental/snapshot.go:diagnosticToStringBuilder
fn diagnostic_to_string_builder(
    program: &Program,
    diagnostic: Option<&Diagnostic>,
    file: NodeId,
    builder: &mut Vec<u8>,
) -> Result<(), Error> {
    let Some(diagnostic) = diagnostic else {
        return Ok(());
    };
    builder.extend_from_slice(b"\n");
    if diagnostic.file != Some(file) {
        let diagnostic_file = diagnostic
            .file
            .expect("a diagnostic of another file than the emitted one names its file");
        builder.extend_from_slice(&tsr_tspath::ensure_path_is_non_module_name(
            &tsr_tspath::relative_from_directory(
                &tsr_tspath::directory(file_path(program, file)?.as_bytes()),
                file_path(program, diagnostic_file)?.as_bytes(),
                b"",
                false,
            ),
        ));
    }
    if diagnostic.file.is_some() {
        builder.extend_from_slice(
            format!("({},{}): ", diagnostic.loc.pos(), diagnostic.loc.len()).as_bytes(),
        );
    }
    builder.extend_from_slice(category_name(diagnostic.category).as_bytes());
    builder.extend_from_slice(format!("{}: ", diagnostic.code).as_bytes());
    builder.extend_from_slice(diagnostic.message_key.as_bytes());
    builder.extend_from_slice(b"\n");
    for arg in &diagnostic.message_args {
        builder.extend_from_slice(arg.as_bytes());
        builder.extend_from_slice(b"\n");
    }
    for chain in &diagnostic.message_chain {
        diagnostic_to_string_builder(program, Some(chain), file, builder)?;
    }
    for info in &diagnostic.related_information {
        diagnostic_to_string_builder(program, Some(info), file, builder)?;
    }
    Ok(())
}

/// `diagnostics.Category.Name()`.
fn category_name(category: i32) -> &'static str {
    match category {
        0 => "warning",
        1 => "error",
        2 => "suggestion",
        3 => "message",
        _ => panic!("Unhandled diagnostic category"),
    }
}

#[cfg(test)]
mod cache_identity_tests {
    use super::*;
    #[test]
    fn diagnostics_identity_tracks_sharing_and_retains_replaced_entry() {
        let snapshot = Snapshot::default();
        let path = JsString::from_bytes(b"/main.ts".as_slice());
        assert!(snapshot
            .cached_semantic_diagnostics_identity(&path)
            .is_none());
        let entry = Arc::new(DiagnosticsOrBuildInfoDiagnosticsWithFileName::default());
        lock(&snapshot.semantic_diagnostics_per_file).insert(path.clone(), entry.clone());
        let old = snapshot
            .cached_semantic_diagnostics_identity(&path)
            .unwrap();
        assert_eq!(
            Some(old.clone()),
            snapshot.cached_semantic_diagnostics_identity(&path)
        );
        lock(&snapshot.semantic_diagnostics_per_file).insert(
            path.clone(),
            Arc::new(DiagnosticsOrBuildInfoDiagnosticsWithFileName::default()),
        );
        assert_ne!(
            Some(old.clone()),
            snapshot.cached_semantic_diagnostics_identity(&path)
        );
        assert_eq!(Arc::strong_count(&entry), 2);
        drop(old);
        assert_eq!(Arc::strong_count(&entry), 1);
    }
}
