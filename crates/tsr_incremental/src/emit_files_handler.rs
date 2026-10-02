//! The incremental program's emit (`emitfileshandler.go`): only the files
//! pending emit, the outputs that are pending for each, the declaration
//! signatures recorded on the way, and the build info after them.
use crate::program::{canceled, file_arc, source_path, Program, SignatureUpdateKind};
use crate::snapshot::{
    get_pending_emit_kind, get_text_handling_source_map_for_signature, lock,
    DiagnosticsOrBuildInfoDiagnosticsWithFileName, EmitSignature, FileEmitKind, Path,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tsr_checker::CheckerRequest;
use tsr_compiler::{combine_emit_results, EmitOnly, EmitOptions, EmitResult, Error, WriteFileData};
use tsr_core::workgroup::WorkGroup;
use tsr_jsstring::JsString;

struct EmitUpdate {
    pending_kind: FileEmitKind,
    result: Option<EmitResult>,
    dts_errors_from_cache: bool,
}

struct EmitFilesHandler<'a> {
    request: &'a CheckerRequest,
    program: &'a Program,
    is_for_dts_errors: bool,
    signatures: Mutex<BTreeMap<Path, JsString>>,
    emit_signatures: Mutex<BTreeMap<Path, EmitSignature>>,
    latest_changed_dts_files: Mutex<BTreeMap<Path, JsString>>,
    deleted_pending_kinds: Mutex<BTreeSet<Path>>,
    emit_updates: Mutex<BTreeMap<Path, EmitUpdate>>,
    has_emit_diagnostics: AtomicBool,
    /// A failure inside a write callback, which can only report a file
    /// system error to the emitter.
    failure: Mutex<Option<Error>>,
}

impl EmitFilesHandler<'_> {
    fn loaded(&self) -> &Arc<tsr_compiler::Program> {
        self.program.program().program()
    }

    fn take_failure(&self) -> Result<(), Error> {
        match lock(&self.failure).take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Determining what all is pending to be emitted based on previous options or previous file emit flags
    // port: tsc/internal/execute/incremental/emitfileshandler.go:emitFilesHandler.getPendingEmitKindForEmitOptions
    fn get_pending_emit_kind_for_emit_options(
        &self,
        emit_kind: FileEmitKind,
        options: &EmitOptions<'_>,
    ) -> FileEmitKind {
        let mut pending_kind = get_pending_emit_kind(emit_kind, FileEmitKind::NONE);
        if options.emit_only == EmitOnly::Dts {
            pending_kind &= FileEmitKind::ALL_DTS;
        }
        if self.is_for_dts_errors {
            pending_kind &= FileEmitKind::DTS_ERRORS;
        }
        pending_kind
    }

    /// Emits the next affected file's emit result (EmitResult and sourceFiles emitted) or returns undefined if iteration is complete
    /// The first of writeFile if provided, writeFile of BuilderProgramHost if provided, writeFile of compiler host
    /// in that order would be used to write the files
    // port: tsc/internal/execute/incremental/emitfileshandler.go:emitFilesHandler.emitAllAffectedFiles
    fn emit_all_affected_files(
        &self,
        options: &EmitOptions<'_>,
    ) -> Result<Option<EmitResult>, Error> {
        // Emit all affected files
        if self.program.snapshot.can_use_incremental_state() {
            let results = self.emit_files_incremental(options)?;
            if self.is_for_dts_errors {
                if let Some(target_source_files) = options.target_source_files {
                    // Result from cache
                    let mut diagnostics = Vec::new();
                    for target_file in target_source_files {
                        let cached = lock(&self.program.snapshot.emit_diagnostics_per_file)
                            .get(&source_path(target_file)?)
                            .cloned();
                        let Some(cached) = cached else {
                            panic!(
                                "runtime error: invalid memory address or nil pointer dereference"
                            );
                        };
                        diagnostics
                            .extend(cached.get_diagnostics(self.loaded(), Some(target_file))?);
                    }
                    let result = EmitResult {
                        emit_skipped: true,
                        diagnostics,
                        ..EmitResult::default()
                    };
                    self.update_has_emit_diagnostics(Some(&result));
                    return Ok(Some(result));
                }
                for result in &results {
                    self.update_has_emit_diagnostics(result.as_ref());
                }
                return Ok(Some(combine_emit_results(results)));
            }
            // Combine results and update buildInfo
            let mut result = combine_emit_results(results);
            self.update_has_emit_diagnostics(Some(&result));
            self.emit_build_info(options, &mut result)?;
            return Ok(Some(result));
        } else if !self.is_for_dts_errors {
            let mut result = self.get_emit_options(options, |options| {
                self.program.program().emit(self.request, options)
            })?;
            self.take_failure()?;
            self.update_has_emit_diagnostics(result.as_ref());
            self.update_snapshot()?;
            if let Some(result) = &mut result {
                self.emit_build_info(options, result)?;
            }
            return Ok(result);
        }
        let checked = self.program.program();
        let diagnostics = match options.target_source_files {
            None => checked.declaration_diagnostics(self.request, None)?,
            Some(target_source_files) => {
                let mut diagnostics = Vec::new();
                for target_source_file in target_source_files {
                    diagnostics.extend(
                        checked.declaration_diagnostics(self.request, Some(target_source_file))?,
                    );
                }
                diagnostics
            }
        };
        let result = EmitResult {
            emit_skipped: true,
            diagnostics,
            ..EmitResult::default()
        };
        if !result.diagnostics.is_empty() {
            self.update_has_emit_diagnostics(Some(&result));
            self.program.snapshot.state().has_emit_diagnostics = true;
        }
        Ok(Some(result))
    }

    // port: tsc/internal/execute/incremental/emitfileshandler.go:emitFilesHandler.updateHasEmitDiagnostics
    fn update_has_emit_diagnostics(&self, result: Option<&EmitResult>) {
        if result.is_some_and(|result| !result.diagnostics.is_empty()) {
            self.has_emit_diagnostics.store(true, Ordering::SeqCst);
        }
    }

    // port: tsc/internal/execute/incremental/emitfileshandler.go:emitFilesHandler.emitBuildInfo
    fn emit_build_info(
        &self,
        options: &EmitOptions<'_>,
        result: &mut EmitResult,
    ) -> Result<(), Error> {
        if let Some(build_info_result) = self.program.emit_build_info(self.request, options)? {
            result.diagnostics.extend(build_info_result.diagnostics);
            result.emitted_files.extend(build_info_result.emitted_files);
        }
        Ok(())
    }

    // port: tsc/internal/execute/incremental/emitfileshandler.go:emitFilesHandler.emitFilesIncremental
    fn emit_files_incremental(
        &self,
        options: &EmitOptions<'_>,
    ) -> Result<Vec<Option<EmitResult>>, Error> {
        // Get all affected files
        crate::affected_files_handler::collect_all_affected_files(self.request, self.program)?;
        if canceled(self.request) {
            return Ok(Vec::new());
        }

        let loaded = self.loaded();
        let pending: Vec<(Path, FileEmitKind)> =
            lock(&self.program.snapshot.affected_files_pending_emit)
                .iter()
                .map(|(path, kind)| (path.clone(), *kind))
                .collect();
        let failure: Mutex<Option<Error>> = Mutex::new(None);
        {
            let wg = WorkGroup::new(loaded.single_threaded());
            for (path, emit_kind) in pending {
                let affected_file = match loaded.file(path.as_bytes()) {
                    Some(file) if loaded.source_file_may_be_emitted(file, false)? => {
                        file_arc(self.program.program(), file)?
                    }
                    _ => {
                        lock(&self.deleted_pending_kinds).insert(path);
                        continue;
                    }
                };
                let pending_kind = self.get_pending_emit_kind_for_emit_options(emit_kind, options);
                if pending_kind != FileEmitKind::NONE {
                    let failure = &failure;
                    wg.queue(move || {
                        let emitted = (|| -> Result<(), Error> {
                            // Determine if we can do partial emit
                            let mut emit_only = EmitOnly::All;
                            if (pending_kind & FileEmitKind::ALL_JS) != FileEmitKind::NONE {
                                emit_only = EmitOnly::Js;
                            }
                            if (pending_kind & FileEmitKind::ALL_DTS) != FileEmitKind::NONE {
                                if emit_only == EmitOnly::Js {
                                    emit_only = EmitOnly::All;
                                } else {
                                    emit_only = EmitOnly::Dts;
                                }
                            }
                            let result = if self.is_for_dts_errors {
                                Some(EmitResult {
                                    emit_skipped: true,
                                    diagnostics: self.program.program().declaration_diagnostics(
                                        self.request,
                                        Some(&affected_file),
                                    )?,
                                    ..EmitResult::default()
                                })
                            } else {
                                let target = std::slice::from_ref(&affected_file);
                                let result = self.get_emit_options(
                                    &EmitOptions {
                                        target_source_files: Some(target),
                                        emit_only,
                                        write_file: options.write_file,
                                        ..EmitOptions::default()
                                    },
                                    |options| self.program.program().emit(self.request, options),
                                )?;
                                self.take_failure()?;
                                result
                            };
                            self.update_has_emit_diagnostics(result.as_ref());

                            // Update the pendingEmit for the file
                            lock(&self.emit_updates).insert(
                                path.clone(),
                                EmitUpdate {
                                    pending_kind: get_pending_emit_kind(emit_kind, pending_kind),
                                    result,
                                    dts_errors_from_cache: false,
                                },
                            );
                            Ok(())
                        })();
                        if let Err(error) = emitted {
                            lock(failure).get_or_insert(error);
                        }
                    });
                }
            }
            wg.run_and_wait();
        }
        if let Some(error) = lock(&failure).take() {
            return Err(error);
        }
        if canceled(self.request) {
            return Ok(Vec::new());
        }

        // Get updated errors that were not included in affected files emit
        let cached: Vec<(Path, Arc<DiagnosticsOrBuildInfoDiagnosticsWithFileName>)> =
            lock(&self.program.snapshot.emit_diagnostics_per_file)
                .iter()
                .map(|(path, diagnostics)| (path.clone(), diagnostics.clone()))
                .collect();
        for (path, diagnostics) in cached {
            if lock(&self.emit_updates).contains_key(&path) {
                continue;
            }
            let affected_file = match loaded.file(path.as_bytes()) {
                Some(file) if loaded.source_file_may_be_emitted(file, false)? => file,
                _ => {
                    lock(&self.deleted_pending_kinds).insert(path);
                    continue;
                }
            };
            let pending_kind = lock(&self.program.snapshot.affected_files_pending_emit)
                .get(&path)
                .copied()
                .unwrap_or_default();
            let diagnostics = diagnostics.get_diagnostics(loaded, Some(affected_file))?;
            lock(&self.emit_updates).insert(
                path,
                EmitUpdate {
                    pending_kind,
                    result: Some(EmitResult {
                        emit_skipped: true,
                        diagnostics,
                        ..EmitResult::default()
                    }),
                    dts_errors_from_cache: true,
                },
            );
        }

        self.update_snapshot()
    }

    /// The emit options of an emit of the program: `options` with a write
    /// callback that records each declaration file's signature and skips a
    /// composite project's unchanged declaration file; `f` runs the emit.
    // port: tsc/internal/execute/incremental/emitfileshandler.go:emitFilesHandler.getEmitOptions
    fn get_emit_options<R>(
        &self,
        options: &EmitOptions<'_>,
        f: impl FnOnce(&EmitOptions<'_>) -> R,
    ) -> R {
        if !self.program.snapshot.options.emit_declarations() {
            return f(options);
        }
        let can_use_incremental_state = self.program.snapshot.can_use_incremental_state();
        let write_file = |file_name: &[u8], text: &[u8], data: &mut WriteFileData| {
            let mut differs_only_in_map = false;
            if tsr_tspath::is_declaration_file_name(file_name) && can_use_incremental_state {
                match self.record_declaration(file_name, text, data, &mut differs_only_in_map) {
                    Ok(true) => return Ok(()),
                    Ok(false) => {}
                    Err(error) => {
                        lock(&self.failure).get_or_insert(error);
                        return Ok(());
                    }
                }
            }

            let host = self.program.host.as_ref();
            let mut a_time = tsr_vfs::iofs::Time::ZERO;
            if differs_only_in_map {
                a_time = host.map_or(tsr_vfs::iofs::Time::ZERO, |host| host.get_mtime(file_name));
            }
            let mut err = if let Some(write_file) = options.write_file {
                write_file(file_name, text, data)
            } else {
                self.loaded().host().write_file(file_name, text)
            };
            if err.is_ok() && differs_only_in_map {
                // Revert the time to original one
                if let Some(host) = host {
                    err = host.set_mtime(file_name, a_time);
                }
            }
            err
        };
        f(&EmitOptions {
            target_source_files: options.target_source_files,
            emit_only: options.emit_only,
            force_emit: options.force_emit,
            write_file: Some(&write_file),
        })
    }

    /// The declaration-file part of `getEmitOptions`'s write callback: the
    /// file's signature, and whether the write is skipped.
    fn record_declaration(
        &self,
        file_name: &[u8],
        text: &[u8],
        data: &mut WriteFileData,
        differs_only_in_map: &mut bool,
    ) -> Result<bool, Error> {
        let source = data
            .source_file
            .expect("a declaration write names its source file");
        let file = self
            .loaded()
            .file_of_node(source)
            .ok_or(tsr_arena::Error::WrongOwner)?
            .clone();
        let path = source_path(&file)?;
        let mut emit_signature = JsString::default();
        let info = lock(&self.program.snapshot.file_infos)
            .get(&path)
            .cloned()
            .expect("an emitted file has its file info");
        if info.signature == info.version {
            let signature = self.program.snapshot.compute_signature_with_diagnostics(
                self.loaded(),
                source,
                text,
                data,
            )?;
            // With d.ts diagnostics they are also part of the signature so emitSignature will be different from it since its just hash of d.ts
            if data.diagnostics.is_empty() {
                emit_signature.clone_from(&signature);
            }
            if signature != info.version {
                // Update it
                lock(&self.signatures).insert(path.clone(), signature);
            }
        }

        // Store d.ts emit hash so later can be compared to check if d.ts has changed.
        // Currently we do this only for composite projects since these are the only projects that can be referenced by other projects
        // and would need their d.ts change time in --build mode
        Ok(self.skip_dts_output_of_composite(
            &path,
            file_name,
            text,
            data,
            emit_signature,
            differs_only_in_map,
        ))
    }

    /// Compare to existing computed signature and store it or handle the changes in d.ts map option from before
    /// returning undefined means that, we dont need to emit this d.ts file since its contents didnt change
    // port: tsc/internal/execute/incremental/emitfileshandler.go:emitFilesHandler.skipDtsOutputOfComposite
    fn skip_dts_output_of_composite(
        &self,
        file_path: &Path,
        output_file_name: &[u8],
        text: &[u8],
        data: &mut WriteFileData,
        mut new_signature: JsString,
        differs_only_in_map: &mut bool,
    ) -> bool {
        if !self.program.snapshot.options.composite.is_true() {
            return false;
        }
        let mut old_signature = JsString::default();
        let old_signature_format = lock(&self.program.snapshot.emit_signatures)
            .get(file_path)
            .cloned();
        if let Some(old_signature_format) = &old_signature_format {
            if old_signature_format.signature.is_empty() {
                old_signature = old_signature_format
                    .signature_with_different_options
                    .as_deref()
                    .unwrap_or_default()[0]
                    .clone();
            } else {
                old_signature.clone_from(&old_signature_format.signature);
            }
        }
        if new_signature.is_empty() {
            new_signature = self
                .program
                .snapshot
                .compute_hash(get_text_handling_source_map_for_signature(text, data));
        }
        // Dont write dts files if they didn't change
        if new_signature == old_signature {
            // If the signature was encoded as string the dts map options match so nothing to do
            if old_signature_format
                .as_ref()
                .is_some_and(|format| format.signature == old_signature)
            {
                data.skipped_dts_write = true;
                return true;
            }
            // Mark as differsOnlyInMap so that we can reverse the timestamp with --build so that
            // the downstream projects dont detect this as change in d.ts file
            *differs_only_in_map = self.program.options().build.is_true();
        } else {
            lock(&self.latest_changed_dts_files)
                .insert(file_path.clone(), JsString::from_bytes(output_file_name));
        }
        lock(&self.emit_signatures).insert(
            file_path.clone(),
            EmitSignature {
                signature: new_signature,
                signature_with_different_options: None,
            },
        );
        false
    }

    // port: tsc/internal/execute/incremental/emitfileshandler.go:emitFilesHandler.updateSnapshot
    fn update_snapshot(&self) -> Result<Vec<Option<EmitResult>>, Error> {
        let snapshot = &self.program.snapshot;
        if snapshot.can_use_incremental_state() {
            for (file, signature) in lock(&self.signatures).iter() {
                if let Some(info) = lock(&snapshot.file_infos).get_mut(file) {
                    info.signature = signature.clone();
                }
                if let Some(testing_data) = &self.program.testing_data {
                    lock(&testing_data.updated_signature_kinds)
                        .insert(file.clone(), SignatureUpdateKind::StoredAtEmit);
                }
                snapshot
                    .build_info_emit_pending
                    .store(true, Ordering::SeqCst);
            }
            for (file, signature) in lock(&self.emit_signatures).iter() {
                lock(&snapshot.emit_signatures).insert(file.clone(), signature.clone());
                snapshot
                    .build_info_emit_pending
                    .store(true, Ordering::SeqCst);
            }
            for file in lock(&self.deleted_pending_kinds).iter() {
                lock(&snapshot.affected_files_pending_emit).remove(file);
                snapshot
                    .build_info_emit_pending
                    .store(true, Ordering::SeqCst);
            }
            // Always use correct order when to collect the result
            let mut results = Vec::new();
            let mut emit_updates = lock(&self.emit_updates);
            for file in self.program.get_source_files() {
                let path = source_path(file)?;
                if let Some(latest_changed_dts_file) =
                    lock(&self.latest_changed_dts_files).get(&path).cloned()
                {
                    let mut state = snapshot.state();
                    state.latest_changed_dts_file = latest_changed_dts_file;
                    snapshot
                        .build_info_emit_pending
                        .store(true, Ordering::SeqCst);
                    state.has_changed_dts_file = true;
                }
                if let Some(update) = emit_updates.remove(&path) {
                    if !update.dts_errors_from_cache {
                        if update.pending_kind == FileEmitKind::NONE {
                            lock(&snapshot.affected_files_pending_emit).remove(&path);
                        } else {
                            lock(&snapshot.affected_files_pending_emit)
                                .insert(path.clone(), update.pending_kind);
                        }
                        snapshot
                            .build_info_emit_pending
                            .store(true, Ordering::SeqCst);
                    }
                    if let Some(result) = update.result {
                        if !result.diagnostics.is_empty() {
                            lock(&snapshot.emit_diagnostics_per_file).insert(
                                path.clone(),
                                Arc::new(
                                    DiagnosticsOrBuildInfoDiagnosticsWithFileName::from_diagnostics(
                                        self.loaded(),
                                        result.diagnostics.clone(),
                                    ),
                                ),
                            );
                        }
                        results.push(Some(result));
                    }
                }
            }
            return Ok(results);
        } else if self.has_emit_diagnostics.load(Ordering::SeqCst) {
            snapshot.state().has_emit_diagnostics = true;
        }
        Ok(Vec::new())
    }
}

/// `None` is the pin's nil result (a canceled emit).
// port: tsc/internal/execute/incremental/emitfileshandler.go:emitFiles
pub(crate) fn emit_files(
    request: &CheckerRequest,
    program: &Program,
    options: &EmitOptions<'_>,
    is_for_dts_errors: bool,
) -> Result<Option<EmitResult>, Error> {
    let emit_handler = EmitFilesHandler {
        request,
        program,
        is_for_dts_errors,
        signatures: Mutex::new(BTreeMap::new()),
        emit_signatures: Mutex::new(BTreeMap::new()),
        latest_changed_dts_files: Mutex::new(BTreeMap::new()),
        deleted_pending_kinds: Mutex::new(BTreeSet::new()),
        emit_updates: Mutex::new(BTreeMap::new()),
        has_emit_diagnostics: AtomicBool::new(false),
        failure: Mutex::new(None),
    };

    // Single file emit - do direct from program
    if !is_for_dts_errors && options.target_source_files.is_some() {
        let result = emit_handler
            .get_emit_options(options, |options| program.program().emit(request, options))?;
        emit_handler.take_failure()?;
        emit_handler.update_has_emit_diagnostics(result.as_ref());
        if canceled(request) {
            return Ok(None);
        }
        emit_handler.update_snapshot()?;
        return Ok(result);
    }

    // Emit only affected files if using builder for emit
    emit_handler.emit_all_affected_files(options)
}
