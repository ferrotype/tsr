//! The files a change affects (`affectedfileshandler.go`): each changed
//! file's new signature, the files whose checking or declaration output it
//! can change, and what that adds to the pending emit.
use crate::program::{canceled, file_arc, source_path, Program, SignatureUpdateKind};
use crate::snapshot::{get_file_emit_kind, lock, FileEmitKind, Path};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tsr_ast::symbol_flags as sf;
use tsr_checker::CheckerRequest;
use tsr_compiler::{EmitOnly, EmitOptions, Error, ProgramFile, WriteFileData};
use tsr_core::workgroup::WorkGroup;
use tsr_jsstring::JsString;

/// The pending emit one affected file's handling adds.
#[derive(Debug, Default)]
pub(crate) struct DtsMayChange(Mutex<BTreeMap<Path, FileEmitKind>>);

impl DtsMayChange {
    // port: tsc/internal/execute/incremental/affectedfileshandler.go:dtsMayChange.addFileToAffectedFilesPendingEmit
    fn add_file_to_affected_files_pending_emit(&self, file_path: &Path, emit_kind: FileEmitKind) {
        lock(&self.0).insert(file_path.clone(), emit_kind);
    }
}

#[derive(Debug)]
struct UpdatedSignatureState {
    signature: JsString,
    kind: SignatureUpdateKind,
}

/// A file's new signature, locked while it is computed.
#[derive(Debug)]
struct UpdatedSignature {
    state: Mutex<UpdatedSignatureState>,
}

/// A run-once operation whose first caller runs it while later callers
/// wait, as `sync.Once` does.
#[derive(Default)]
struct Once(Mutex<bool>);

impl Once {
    fn call(&self, f: impl FnOnce() -> Result<(), Error>) -> Result<(), Error> {
        let mut done = lock(&self.0);
        if *done {
            return Ok(());
        }
        *done = true;
        f()
    }
}

struct AffectedFilesHandler<'a> {
    request: &'a CheckerRequest,
    program: &'a Program,
    has_all_files_excluding_default_library_file: AtomicBool,
    updated_signatures: Mutex<BTreeMap<Path, Arc<UpdatedSignature>>>,
    dts_may_change: Mutex<Vec<Arc<DtsMayChange>>>,
    files_to_remove_diagnostics: Mutex<BTreeSet<Path>>,
    cleaned_diagnostics_of_lib_files: Once,
    seen_file_and_references: Mutex<BTreeMap<Path, bool>>,
}

/// The walk's callback over one referencing file and its path.
type ReferencedByVisitor<'a> =
    dyn FnMut(Option<&Arc<ProgramFile>>, &Path) -> Result<Visit, Error> + 'a;

/// What the walk over a file's referencing files does with one of them.
struct Visit {
    queue_for_file: bool,
    fast_return: bool,
}

impl AffectedFilesHandler<'_> {
    fn loaded(&self) -> &Arc<tsr_compiler::Program> {
        self.program.program().program()
    }

    // port: tsc/internal/execute/incremental/affectedfileshandler.go:affectedFilesHandler.getDtsMayChange
    fn get_dts_may_change(
        &self,
        affected_file_path: &Path,
        affected_file_emit_kind: FileEmitKind,
    ) -> Arc<DtsMayChange> {
        let result = Arc::new(DtsMayChange::default());
        result.add_file_to_affected_files_pending_emit(affected_file_path, affected_file_emit_kind);
        lock(&self.dts_may_change).push(result.clone());
        result
    }

    // port: tsc/internal/execute/incremental/affectedfileshandler.go:affectedFilesHandler.isChangedSignature
    fn is_changed_signature(&self, path: &Path) -> bool {
        let new_signature = lock(&self.updated_signatures)
            .get(path)
            .cloned()
            .expect("the signature of a handled file was updated");
        // This method is called after updating signatures of that path, so signature is present in updatedSignatures
        // And is already calculated, so no need to lock and unlock mutex on the entry
        let old_info = lock(&self.program.snapshot.file_infos)
            .get(path)
            .cloned()
            .expect("a handled file has its file info");
        let changed = lock(&new_signature.state).signature != old_info.signature;
        changed
    }

    // port: tsc/internal/execute/incremental/affectedfileshandler.go:affectedFilesHandler.removeSemanticDiagnosticsOf
    fn remove_semantic_diagnostics_of(&self, path: &Path) {
        lock(&self.files_to_remove_diagnostics).insert(path.clone());
    }

    // port: tsc/internal/execute/incremental/affectedfileshandler.go:affectedFilesHandler.removeDiagnosticsOfLibraryFiles
    fn remove_diagnostics_of_library_files(&self) -> Result<(), Error> {
        self.cleaned_diagnostics_of_lib_files.call(|| {
            let loaded = self.loaded();
            for file in self.program.get_source_files() {
                let path = source_path(file)?;
                if loaded.is_lib(path.as_bytes()) && !loaded.skip_type_checking(file, true)? {
                    self.remove_semantic_diagnostics_of(&path);
                }
            }
            Ok(())
        })
    }

    // port: tsc/internal/execute/incremental/affectedfileshandler.go:affectedFilesHandler.computeDtsSignature
    fn compute_dts_signature(&self, file: &Arc<ProgramFile>) -> Result<JsString, Error> {
        let signature = Mutex::new(JsString::default());
        let failure = Mutex::new(None);
        let _done = self.program.begin_nested_emit();
        let checked = self.program.program();
        let write_file = |file_name: &[u8], text: &[u8], data: &mut WriteFileData| {
            assert!(
                tsr_tspath::is_declaration_file_name(file_name),
                "File extension for signature expected to be dts, got : {}",
                String::from_utf8_lossy(file_name)
            );
            match self.program.snapshot.compute_signature_with_diagnostics(
                checked.program(),
                file.source(),
                text,
                data,
            ) {
                Ok(computed) => *lock(&signature) = computed,
                Err(error) => {
                    lock(&failure).get_or_insert(error);
                }
            }
            Ok(())
        };
        checked.emit(
            self.request,
            &EmitOptions {
                target_source_files: Some(std::slice::from_ref(file)),
                emit_only: EmitOnly::BuilderSignature,
                write_file: Some(&write_file),
                ..EmitOptions::default()
            },
        )?;
        if let Some(error) = failure
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            return Err(error);
        }
        Ok(signature
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner))
    }

    // port: tsc/internal/execute/incremental/affectedfileshandler.go:affectedFilesHandler.updateShapeSignature
    fn update_shape_signature(
        &self,
        file: &Arc<ProgramFile>,
        use_file_version_as_signature: bool,
    ) -> Result<bool, Error> {
        let path = source_path(file)?;
        let update = Arc::new(UpdatedSignature {
            state: Mutex::new(UpdatedSignatureState {
                signature: JsString::default(),
                kind: SignatureUpdateKind::ComputedDts,
            }),
        });
        let mut update_state = lock(&update.state);
        // If we have cached the result for this file, that means hence forth we should assume file shape is uptodate
        let existing = {
            let mut updated_signatures = lock(&self.updated_signatures);
            if let Some(existing) = updated_signatures.get(&path) {
                Some(existing.clone())
            } else {
                updated_signatures.insert(path.clone(), update.clone());
                None
            }
        };
        if let Some(existing) = existing {
            // Ensure calculations for existing ones are complete before using the value
            drop(update_state);
            let _existing = lock(&existing.state);
            return Ok(false);
        }

        let info = lock(&self.program.snapshot.file_infos)
            .get(&path)
            .cloned()
            .expect("a program file has its file info");
        let prev_signature = info.signature.clone();
        // JSON files have no declaration output from which to compute a shape
        // signature, so use the file version to conservatively invalidate dependents.
        let source = file.bound().view().source_file()?;
        if !source.is_declaration_file
            && !tsr_ast::utilities::is_json_source_file(&source)
            && !use_file_version_as_signature
        {
            update_state.signature = self.compute_dts_signature(file)?;
        }
        // Default is to use file version as signature
        if update_state.signature.is_empty() {
            update_state.signature = info.version.clone();
            update_state.kind = SignatureUpdateKind::UsedVersion;
        }
        Ok(update_state.signature != prev_signature)
    }

    // port: tsc/internal/execute/incremental/affectedfileshandler.go:affectedFilesHandler.getFilesAffectedBy
    fn get_files_affected_by(&self, path: &Path) -> Result<Vec<Arc<ProgramFile>>, Error> {
        let loaded = self.loaded();
        let Some(file) = loaded.file(path.as_bytes()) else {
            return Ok(Vec::new());
        };
        let file = file_arc(self.program.program(), file)?;

        if !self.update_shape_signature(&file, false)? {
            return Ok(vec![file]);
        }

        let file_path = source_path(&file)?;
        if lock(&self.program.snapshot.file_infos)
            .get(&file_path)
            .is_some_and(|info| info.affects_global_scope)
        {
            self.has_all_files_excluding_default_library_file
                .store(true, Ordering::SeqCst);
            return Ok(self
                .program
                .snapshot
                .get_all_files_excluding_default_library_file(loaded, Some(&file))?
                .to_vec());
        }

        if self.program.snapshot.options.isolated_modules.is_true() {
            return Ok(vec![file]);
        }

        // Now we need to if each file in the referencedBy list has a shape change as well.
        // Because if so, its own referencedBy files need to be saved as well to make the
        // emitting result consistent with files on disk.
        let seen_file_names_map =
            self.for_each_file_referenced_by(&file, &mut |current_file, _| {
                // If the current file is not nil and has a shape change, we need to queue it for processing
                if let Some(current_file) = current_file {
                    if self.update_shape_signature(current_file, false)? {
                        return Ok(Visit {
                            queue_for_file: true,
                            fast_return: false,
                        });
                    }
                }
                Ok(Visit {
                    queue_for_file: false,
                    fast_return: false,
                })
            })?;
        // Return array of values that needs emit
        Ok(seen_file_names_map.into_values().flatten().collect())
    }

    // port: tsc/internal/execute/incremental/affectedfileshandler.go:affectedFilesHandler.forEachFileReferencedBy
    fn for_each_file_referenced_by(
        &self,
        file: &Arc<ProgramFile>,
        f: &mut ReferencedByVisitor<'_>,
    ) -> Result<BTreeMap<Path, Option<Arc<ProgramFile>>>, Error> {
        // Now we need to if each file in the referencedBy list has a shape change as well.
        // Because if so, its own referencedBy files need to be saved as well to make the
        // emitting result consistent with files on disk.
        let mut seen_file_names_map = BTreeMap::new();
        // Start with the paths this file was referenced by
        let file_path = source_path(file)?;
        seen_file_names_map.insert(file_path.clone(), Some(file.clone()));
        let mut queue = self
            .program
            .snapshot
            .referenced_map
            .get_referenced_by(&file_path);
        while let Some(current_path) = queue.pop() {
            if !seen_file_names_map.contains_key(&current_path) {
                let current_file = self
                    .loaded()
                    .file(current_path.as_bytes())
                    .map(|current| file_arc(self.program.program(), current))
                    .transpose()?;
                seen_file_names_map.insert(current_path.clone(), current_file.clone());
                let visit = f(current_file.as_ref(), &current_path)?;
                if visit.fast_return {
                    return Ok(seen_file_names_map);
                }
                if visit.queue_for_file {
                    queue.extend(
                        self.program
                            .snapshot
                            .referenced_map
                            .get_referenced_by(&current_path),
                    );
                }
            }
        }
        Ok(seen_file_names_map)
    }

    /// Handles semantic diagnostics and dts emit for affectedFile and files, that are referencing modules that export entities from affected file
    /// This is because even though js emit doesnt change, dts emit / type used can change resulting in need for dts emit and js change
    // port: tsc/internal/execute/incremental/affectedfileshandler.go:affectedFilesHandler.handleDtsMayChangeOfAffectedFile
    fn handle_dts_may_change_of_affected_file(
        &self,
        dts_may_change: &DtsMayChange,
        affected_file: &Arc<ProgramFile>,
    ) -> Result<(), Error> {
        let affected_path = source_path(affected_file)?;
        self.remove_semantic_diagnostics_of(&affected_path);

        // If affected files is everything except default library, then nothing more to do
        if self
            .has_all_files_excluding_default_library_file
            .load(Ordering::SeqCst)
        {
            self.remove_diagnostics_of_library_files()?;
            // When a change affects the global scope, all files are considered to be affected without updating their signature
            // That means when affected file is handled, its signature can be out of date
            // To avoid this, ensure that we update the signature for any affected file in this scenario.
            self.update_shape_signature(affected_file, false)?;
            return Ok(());
        }

        let options = &self.program.snapshot.options;
        if options
            .assume_changes_only_affect_direct_dependencies
            .is_true()
        {
            return Ok(());
        }

        // Iterate on referencing modules that export entities from affected file and delete diagnostics and add pending emit
        // If there was change in signature (dts output) for the changed file,
        // then only we need to handle pending file emit
        if !lock(&self.program.snapshot.changed_files_set).contains(&affected_path)
            || !self.is_changed_signature(&affected_path)
        {
            return Ok(());
        }

        // At this point affectedFile is actually one of the changed files
        // that has some change in its .d.ts signature.

        // Since isolated modules dont change js files, files affected by change in signature is itself
        // But we need to cleanup semantic diagnostics and queue dts emit for affected files
        if options.isolated_modules.is_true() {
            self.for_each_file_referenced_by(affected_file, &mut |_, current_path| {
                if self.handle_dts_may_change_of_global_scope(
                    dts_may_change,
                    current_path,
                    /*invalidateJsFiles*/ false,
                )? {
                    return Ok(Visit {
                        queue_for_file: false,
                        fast_return: true,
                    });
                }
                self.handle_dts_may_change_of(
                    dts_may_change,
                    current_path,
                    /*invalidateJsFiles*/ false,
                )?;
                Ok(Visit {
                    queue_for_file: self.is_changed_signature(current_path),
                    fast_return: false,
                })
            })?;
        }

        let mut invalidate_js_files = false;
        // If exported const enum, we need to ensure that js files are emitted as well since the const enum value changed
        let file_symbol = affected_file
            .bound()
            .view()
            .node_binding(affected_file.source())?
            .and_then(|binding| binding.symbol);
        if let Some(file_symbol) = file_symbol {
            let loaded = self.loaded();
            self.program
                .program()
                .with_type_checker_for_file_exclusive(
                    self.request,
                    affected_file.source(),
                    &mut |type_checker| {
                        let symbol = type_checker.symbol_ref(file_symbol)?;
                        let Some(exports) = type_checker.symbol(symbol)?.exports() else {
                            return Ok(());
                        };
                        let exported: Vec<_> = type_checker
                            .symbol_table(exports)?
                            .iter()
                            .filter_map(|(_, symbol)| symbol)
                            .collect();
                        for exported in exported {
                            let exported = type_checker.symbol_ref(exported)?;
                            if type_checker.symbol(exported)?.flags() & sf::CONST_ENUM != 0 {
                                invalidate_js_files = true;
                                break;
                            }
                            let aliased = type_checker.skip_alias(exported)?;
                            if aliased == exported {
                                continue;
                            }
                            if type_checker.symbol(aliased)?.flags() & sf::CONST_ENUM != 0
                                && type_checker
                                    .symbol_declarations(aliased)?
                                    .iter()
                                    .flatten()
                                    .any(|d| {
                                        loaded.file_of_node(d).is_some_and(|file| {
                                            file.source() == affected_file.source()
                                        })
                                    })
                            {
                                invalidate_js_files = true;
                                break;
                            }
                        }
                        Ok(())
                    },
                )?;
        }

        // Go through files that reference affected file and handle dts emit and semantic diagnostics for them and their references
        for file_referencing_changed_file in self
            .program
            .snapshot
            .referenced_map
            .get_referenced_by(&affected_path)
        {
            if self.handle_dts_may_change_of_global_scope(
                dts_may_change,
                &file_referencing_changed_file,
                invalidate_js_files,
            )? {
                return Ok(());
            }
            // Since references of changed file = affected files - we would have already handled d.ts emit and semantic diagnostics
            // for those files. Now we need to handle files referencing those affected files to ensure correctness.
            for file_referencing_affected_file in self
                .program
                .snapshot
                .referenced_map
                .get_referenced_by(&file_referencing_changed_file)
            {
                if self.handle_dts_may_change_of_file_and_references(
                    dts_may_change,
                    &file_referencing_affected_file,
                    invalidate_js_files,
                )? {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    // port: tsc/internal/execute/incremental/affectedfileshandler.go:affectedFilesHandler.handleDtsMayChangeOfFileAndReferences
    fn handle_dts_may_change_of_file_and_references(
        &self,
        dts_may_change: &DtsMayChange,
        file_path: &Path,
        invalidate_js_files: bool,
    ) -> Result<bool, Error> {
        {
            let mut seen = lock(&self.seen_file_and_references);
            match seen.get(file_path).copied() {
                Some(existing) if existing || !invalidate_js_files => return Ok(false),
                Some(_) => {
                    seen.insert(file_path.clone(), true);
                }
                None => {
                    seen.insert(file_path.clone(), invalidate_js_files);
                }
            }
        }

        if self.handle_dts_may_change_of_global_scope(
            dts_may_change,
            file_path,
            invalidate_js_files,
        )? {
            return Ok(true);
        }
        self.handle_dts_may_change_of(dts_may_change, file_path, invalidate_js_files)?;

        // Remove the diagnostics of files that import this file and
        // any files that are referenced by it (directly or indirectly)
        for referencing_file_path in self
            .program
            .snapshot
            .referenced_map
            .get_referenced_by(file_path)
        {
            if self.handle_dts_may_change_of_file_and_references(
                dts_may_change,
                &referencing_file_path,
                invalidate_js_files,
            )? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    // port: tsc/internal/execute/incremental/affectedfileshandler.go:affectedFilesHandler.handleDtsMayChangeOfGlobalScope
    fn handle_dts_may_change_of_global_scope(
        &self,
        dts_may_change: &DtsMayChange,
        file_path: &Path,
        invalidate_js_files: bool,
    ) -> Result<bool, Error> {
        if !lock(&self.program.snapshot.file_infos)
            .get(file_path)
            .is_some_and(|info| info.affects_global_scope)
        {
            return Ok(false);
        }
        // Every file needs to be handled
        let files = self
            .program
            .snapshot
            .get_all_files_excluding_default_library_file(self.loaded(), None)?
            .to_vec();
        for file in &files {
            self.handle_dts_may_change_of(
                dts_may_change,
                &source_path(file)?,
                invalidate_js_files,
            )?;
        }
        self.remove_diagnostics_of_library_files()?;
        Ok(true)
    }

    /// Handle the dts may change, so they need to be added to pending emit if dts emit is enabled,
    /// Also we need to make sure signature is updated for these files
    // port: tsc/internal/execute/incremental/affectedfileshandler.go:affectedFilesHandler.handleDtsMayChangeOf
    fn handle_dts_may_change_of(
        &self,
        dts_may_change: &DtsMayChange,
        path: &Path,
        invalidate_js_files: bool,
    ) -> Result<(), Error> {
        if lock(&self.program.snapshot.changed_files_set).contains(path) {
            return Ok(());
        }
        let Some(file) = self.loaded().file(path.as_bytes()) else {
            return Ok(());
        };
        let file = file_arc(self.program.program(), file)?;
        self.remove_semantic_diagnostics_of(path);
        // Even though the js emit doesnt change and we are already handling dts emit and semantic diagnostics
        // we need to update the signature to reflect correctness of the signature(which is output d.ts emit) of this file
        // This ensures that we dont later during incremental builds considering wrong signature.
        // Eg where this also is needed to ensure that .tsbuildinfo generated by incremental build should be same as if it was first fresh build
        // But we avoid expensive full shape computation, as using file version as shape is enough for correctness.
        self.update_shape_signature(&file, true)?;
        // If not dts emit, nothing more to do
        let options = &self.program.snapshot.options;
        if invalidate_js_files {
            dts_may_change
                .add_file_to_affected_files_pending_emit(path, get_file_emit_kind(options));
        } else if options.emit_declarations() {
            dts_may_change.add_file_to_affected_files_pending_emit(
                path,
                if options.declaration_map.is_true() {
                    FileEmitKind::ALL_DTS
                } else {
                    FileEmitKind::DTS
                },
            );
        }
        Ok(())
    }

    // port: tsc/internal/execute/incremental/affectedfileshandler.go:affectedFilesHandler.updateSnapshot
    fn update_snapshot(&self) {
        if canceled(self.request) {
            return;
        }
        let snapshot = &self.program.snapshot;
        for (file_path, update) in lock(&self.updated_signatures).iter() {
            let update = lock(&update.state);
            if let Some(info) = lock(&snapshot.file_infos).get_mut(file_path) {
                info.signature = update.signature.clone();
                if let Some(testing_data) = &self.program.testing_data {
                    lock(&testing_data.updated_signature_kinds)
                        .insert(file_path.clone(), update.kind);
                }
            }
        }
        for file in lock(&self.files_to_remove_diagnostics).iter() {
            lock(&snapshot.semantic_diagnostics_per_file).remove(file);
        }
        for change in lock(&self.dts_may_change).iter() {
            for (file_path, &emit_kind) in lock(&change.0).iter() {
                snapshot.add_file_to_affected_files_pending_emit(file_path, emit_kind);
            }
        }
        lock(&snapshot.changed_files_set).clear();
        snapshot
            .build_info_emit_pending
            .store(true, Ordering::SeqCst);
    }
}

// port: tsc/internal/execute/incremental/affectedfileshandler.go:collectAllAffectedFiles
pub(crate) fn collect_all_affected_files(
    request: &CheckerRequest,
    program: &Program,
) -> Result<(), Error> {
    let changed_files: Vec<Path> = lock(&program.snapshot.changed_files_set)
        .iter()
        .cloned()
        .collect();
    if changed_files.is_empty() {
        return Ok(());
    }

    let handler = AffectedFilesHandler {
        request,
        program,
        has_all_files_excluding_default_library_file: AtomicBool::new(false),
        updated_signatures: Mutex::new(BTreeMap::new()),
        dts_may_change: Mutex::new(Vec::new()),
        files_to_remove_diagnostics: Mutex::new(BTreeSet::new()),
        cleaned_diagnostics_of_lib_files: Once::default(),
        seen_file_and_references: Mutex::new(BTreeMap::new()),
    };
    let single_threaded = program.program().program().single_threaded();
    let failure: Mutex<Option<Error>> = Mutex::new(None);
    let result: Mutex<BTreeMap<Path, Arc<ProgramFile>>> = Mutex::new(BTreeMap::new());
    {
        let wg = WorkGroup::new(single_threaded);
        for file in &changed_files {
            let (handler, result, failure) = (&handler, &result, &failure);
            wg.queue(move || match handler.get_files_affected_by(file) {
                Ok(affected_files) => {
                    for affected_file in affected_files {
                        if let Ok(path) = source_path(&affected_file) {
                            lock(result).insert(path, affected_file);
                        }
                    }
                }
                Err(error) => {
                    lock(failure).get_or_insert(error);
                }
            });
        }
        wg.run_and_wait();
    }
    if let Some(error) = lock(&failure).take() {
        return Err(error);
    }

    if canceled(request) {
        return Ok(());
    }

    // For all the affected files, get all the files that would need to change their dts or js files,
    // update their diagnostics
    {
        let wg = WorkGroup::new(single_threaded);
        let emit_kind = get_file_emit_kind(&program.snapshot.options);
        let affected: Vec<(Path, Arc<ProgramFile>)> = lock(&result)
            .iter()
            .map(|(path, file)| (path.clone(), file.clone()))
            .collect();
        for (path, file) in affected {
            // remove the cached semantic diagnostics and handle dts emit and js emit if needed
            let dts_may_change = handler.get_dts_may_change(&path, emit_kind);
            let (handler, failure) = (&handler, &failure);
            wg.queue(move || {
                if let Err(error) =
                    handler.handle_dts_may_change_of_affected_file(&dts_may_change, &file)
                {
                    lock(failure).get_or_insert(error);
                }
            });
        }
        wg.run_and_wait();
    }
    if let Some(error) = lock(&failure).take() {
        return Err(error);
    }

    // Update the snapshot with the new state
    handler.update_snapshot();
    Ok(())
}
