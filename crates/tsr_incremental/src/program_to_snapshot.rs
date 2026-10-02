//! A program's incremental state, from the old program's where there is one
//! (`programtosnapshot.go`): each file's version and references, what
//! changed, which cached diagnostics and emit signatures carry over, and
//! what is pending emit.
use crate::program::Program;
use crate::reference_map::ReferenceSet;
use crate::snapshot::{
    get_file_emit_kind, get_pending_emit_kind_with_options, lock, BuildInfoDiagnosticWithFileName,
    DiagnosticsOrBuildInfoDiagnosticsWithFileName, FileEmitKind, FileInfo, Path,
    ProgramDiagnostics, Snapshot,
};
use std::collections::BTreeSet;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use tsr_ast::{Diagnostic, SyntaxKind as K};
use tsr_checker::{CheckerRequest, Operation};
use tsr_compiler::{CheckedProgram, Error, ProgramFile};
use tsr_core::workgroup::WorkGroup;
use tsr_jsstring::JsString;

// port: tsc/internal/execute/incremental/programtosnapshot.go:programToSnapshot
pub(crate) fn program_to_snapshot(
    program: &Arc<CheckedProgram>,
    old_program: Option<&Program>,
    hash_with_text: bool,
) -> Result<Arc<Snapshot>, Error> {
    if let Some(old_program) = old_program {
        if old_program
            .program
            .as_ref()
            .is_some_and(|old| Arc::ptr_eq(old, program))
        {
            return Ok(old_program.snapshot.clone());
        }
    }
    let snapshot = Snapshot {
        options: Arc::new(program.program().options().clone()),
        hash_with_text,
        ..Snapshot::default()
    };
    snapshot.state().check_pending = program.program().options().no_check.is_true();
    let mut to = ToProgramSnapshot {
        program,
        old_program,
        snapshot,
        global_file_removed: false,
    };

    if to.snapshot.can_use_incremental_state() {
        to.reuse_from_old_program();
        to.compute_program_file_changes()?;
        to.handle_file_delete()?;
        to.handle_global_scope_change()?;
        to.handle_pending_emit()?;
        to.handle_pending_check();
    }
    Ok(Arc::new(to.snapshot))
}

struct ToProgramSnapshot<'a> {
    program: &'a Arc<CheckedProgram>,
    old_program: Option<&'a Program>,
    snapshot: Snapshot,
    global_file_removed: bool,
}

fn source_path(file: &ProgramFile) -> Result<Path, Error> {
    Ok(file
        .bound()
        .view()
        .source_file()?
        .parse_options()
        .path
        .clone())
}

impl ToProgramSnapshot<'_> {
    // port: tsc/internal/execute/incremental/programtosnapshot.go:toProgramSnapshot.reuseFromOldProgram
    fn reuse_from_old_program(&mut self) {
        if let Some(old_program) = self.old_program {
            let old_snapshot = &old_program.snapshot;
            let old_state = old_snapshot.state().clone();
            let mut state = self.snapshot.state();
            if self.snapshot.options.composite.is_true() {
                state.latest_changed_dts_file = old_state.latest_changed_dts_file.clone();
            }
            // Copy old snapshot's changed files set
            for key in lock(&old_snapshot.changed_files_set).iter() {
                lock(&self.snapshot.changed_files_set).insert(key.clone());
            }
            for (key, &emit_kind) in lock(&old_snapshot.affected_files_pending_emit).iter() {
                lock(&self.snapshot.affected_files_pending_emit).insert(key.clone(), emit_kind);
            }
            self.snapshot.build_info_emit_pending.store(
                old_snapshot.build_info_emit_pending.load(Ordering::SeqCst),
                Ordering::SeqCst,
            );
            state.has_errors_from_old_state = old_state.has_errors;
            state.has_semantic_errors_from_old_state = old_state.has_semantic_errors;
            state.package_jsons_from_old_state = old_state.package_jsons;
            state.missing_package_jsons_from_old_state = old_state.missing_package_jsons;
        } else {
            self.snapshot
                .build_info_emit_pending
                .store(self.snapshot.options.is_incremental(), Ordering::SeqCst);
        }
    }

    // port: tsc/internal/execute/incremental/programtosnapshot.go:toProgramSnapshot.computeProgramFileChanges
    fn compute_program_file_changes(&mut self) -> Result<(), Error> {
        let program = self.program;
        let loaded = program.program();
        let old_snapshot = self.old_program.map(|old| old.snapshot.as_ref());
        let can_copy_semantic_diagnostics = old_snapshot.is_some_and(|old| {
            !tsr_tsoptions::affects::compiler_options_affect_semantic_diagnostics(
                Some(&old.options),
                Some(loaded.options()),
            )
        });
        // We can only reuse emit signatures (i.e. .d.ts signatures) if the .d.ts file is unchanged,
        // which will eg be depedent on change in options like declarationDir and outDir options are unchanged.
        // We need to look in oldState.compilerOptions, rather than oldCompilerOptions (i.e.we need to disregard useOldState) because
        // oldCompilerOptions can be undefined if there was change in say module from None to some other option
        // which would make useOldState as false since we can now use reference maps that are needed to track what to emit, what to check etc
        // but that option change does not affect d.ts file name so emitSignatures should still be reused.
        let can_copy_emit_signatures = self.snapshot.options.composite.is_true()
            && old_snapshot.is_some_and(|old| {
                !tsr_tsoptions::affects::compiler_options_affect_declaration_path(
                    Some(&old.options),
                    Some(loaded.options()),
                )
            });
        let copy_declaration_file_diagnostics = can_copy_semantic_diagnostics
            && old_snapshot.is_some_and(|old| {
                self.snapshot.options.skip_lib_check.is_true()
                    == old.options.skip_lib_check.is_true()
            });
        let copy_lib_file_diagnostics = copy_declaration_file_diagnostics
            && old_snapshot.is_some_and(|old| {
                self.snapshot.options.skip_default_lib_check.is_true()
                    == old.options.skip_default_lib_check.is_true()
            });

        let files = loaded.files();
        let failure: Mutex<Option<Error>> = Mutex::new(None);
        let snapshot = &self.snapshot;
        let wg = WorkGroup::new(loaded.single_threaded());
        for file in files {
            let failure = &failure;
            wg.queue(move || {
                let result = (|| -> Result<(), Error> {
                    let source = file.bound().view().source_file()?;
                    let path = source.parse_options().path.clone();
                    let version_text = if source.content_mapper().is_empty() {
                        source.text().as_bytes().to_vec()
                    } else {
                        let mut text = source.original_text().to_vec();
                        text.push(0);
                        text.extend_from_slice(source.content_mapper_transform_identity());
                        text
                    };
                    let version = snapshot.compute_hash(&version_text);
                    let implied_node_format = loaded
                        .metadata(path.as_bytes())
                        .map(|meta| meta.implied_node_format)
                        .unwrap_or_default();
                    let affects_global_scope = file_affects_global_scope(file)?;
                    let mut signature = JsString::default();
                    let new_references = get_referenced_files(program, file)?;
                    if let Some(new_references) = &new_references {
                        snapshot
                            .referenced_map
                            .store_references(&path, new_references.clone());
                    }
                    if let Some(old_snapshot) = old_snapshot {
                        let old_file_info = lock(&old_snapshot.file_infos).get(&path).cloned();
                        if let Some(old_file_info) = old_file_info {
                            signature = old_file_info.signature.clone();
                            let old_references = old_snapshot.referenced_map.get_references(&path);
                            if old_file_info.version != version
                                || old_file_info.affects_global_scope != affects_global_scope
                                || old_file_info.implied_node_format != implied_node_format
                            {
                                snapshot.add_file_to_change_set(&path);
                            } else if !references_equal(
                                new_references.as_ref(),
                                old_references.as_ref(),
                            ) {
                                // Referenced files changed
                                snapshot.add_file_to_change_set(&path);
                            } else if let Some(new_references) = &new_references {
                                for ref_path in new_references.iter() {
                                    if loaded.file(ref_path.as_bytes()).is_none()
                                        && lock(&old_snapshot.file_infos).contains_key(ref_path)
                                    {
                                        // Referenced file was deleted in the new program
                                        snapshot.add_file_to_change_set(&path);
                                        break;
                                    }
                                }
                            }
                        } else {
                            snapshot.add_file_to_change_set(&path);
                        }
                        if !lock(&snapshot.changed_files_set).contains(&path) {
                            let emit_diagnostics = lock(&old_snapshot.emit_diagnostics_per_file)
                                .get(&path)
                                .cloned();
                            if let Some(emit_diagnostics) = emit_diagnostics {
                                let repopulated = repopulate_diagnostics_of_file(
                                    &emit_diagnostics,
                                    loaded,
                                    file,
                                )?;
                                lock(&snapshot.emit_diagnostics_per_file)
                                    .insert(path.clone(), repopulated);
                            }
                            if can_copy_semantic_diagnostics
                                && (!source.is_declaration_file
                                    || copy_declaration_file_diagnostics)
                                && (!loaded.is_lib(path.as_bytes()) || copy_lib_file_diagnostics)
                            {
                                // Unchanged file copy diagnostics
                                let diagnostics = lock(&old_snapshot.semantic_diagnostics_per_file)
                                    .get(&path)
                                    .cloned();
                                if let Some(diagnostics) = diagnostics {
                                    let repopulated =
                                        repopulate_diagnostics_of_file(&diagnostics, loaded, file)?;
                                    lock(&snapshot.semantic_diagnostics_per_file)
                                        .insert(path.clone(), repopulated);
                                }
                            }
                        }
                        if can_copy_emit_signatures {
                            let old_emit_signature =
                                lock(&old_snapshot.emit_signatures).get(&path).cloned();
                            if let Some(old_emit_signature) = old_emit_signature {
                                lock(&snapshot.emit_signatures).insert(
                                    path.clone(),
                                    old_emit_signature.get_new_emit_signature(
                                        &old_snapshot.options,
                                        &snapshot.options,
                                    ),
                                );
                            }
                        }
                    } else {
                        snapshot.add_file_to_affected_files_pending_emit(
                            &path,
                            get_file_emit_kind(&snapshot.options),
                        );
                        signature.clone_from(&version);
                    }
                    lock(&snapshot.file_infos).insert(
                        path,
                        FileInfo {
                            version,
                            signature,
                            affects_global_scope,
                            implied_node_format,
                        },
                    );
                    Ok(())
                })();
                if let Err(error) = result {
                    lock(failure).get_or_insert(error);
                }
            });
        }
        wg.run_and_wait();
        drop(wg);
        match failure
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    // port: tsc/internal/execute/incremental/programtosnapshot.go:toProgramSnapshot.handleFileDelete
    fn handle_file_delete(&mut self) -> Result<(), Error> {
        if let Some(old_program) = self.old_program {
            // If the global file is removed, add all files as changed
            let old_file_infos = lock(&old_program.snapshot.file_infos).clone();
            for (file_path, old_info) in &old_file_infos {
                if !lock(&self.snapshot.file_infos).contains_key(file_path) {
                    if old_info.affects_global_scope {
                        let files = self
                            .snapshot
                            .get_all_files_excluding_default_library_file(
                                self.program.program(),
                                None,
                            )?
                            .to_vec();
                        for file in &files {
                            self.snapshot.add_file_to_change_set(&source_path(file)?);
                        }
                        self.global_file_removed = true;
                    } else {
                        self.snapshot
                            .build_info_emit_pending
                            .store(true, Ordering::SeqCst);
                    }
                    break;
                }
            }
        }
        Ok(())
    }

    // port: tsc/internal/execute/incremental/programtosnapshot.go:toProgramSnapshot.handleGlobalScopeChange
    fn handle_global_scope_change(&mut self) -> Result<(), Error> {
        let Some(old_program) = self.old_program else {
            return Ok(());
        };
        if self.global_file_removed {
            return Ok(());
        }
        let mut global_scope_lost = false;
        for (file_path, old_info) in lock(&old_program.snapshot.file_infos).iter() {
            if !old_info.affects_global_scope {
                continue;
            }
            if lock(&self.snapshot.file_infos)
                .get(file_path)
                .is_some_and(|new_info| !new_info.affects_global_scope)
            {
                global_scope_lost = true;
                break;
            }
        }
        if global_scope_lost {
            let files = self
                .snapshot
                .get_all_files_excluding_default_library_file(self.program.program(), None)?
                .to_vec();
            for file in &files {
                self.snapshot.add_file_to_change_set(&source_path(file)?);
            }
        }
        Ok(())
    }

    // port: tsc/internal/execute/incremental/programtosnapshot.go:toProgramSnapshot.handlePendingEmit
    fn handle_pending_emit(&mut self) -> Result<(), Error> {
        if let Some(old_program) = self.old_program.filter(|_| !self.global_file_removed) {
            // If options affect emit, then we need to do complete emit per compiler options
            // otherwise only the js or dts that needs to emitted because its different from previously emitted options
            let pending_emit_kind = if tsr_tsoptions::affects::compiler_options_affect_emit(
                Some(&old_program.snapshot.options),
                Some(&self.snapshot.options),
            ) {
                get_file_emit_kind(&self.snapshot.options)
            } else {
                get_pending_emit_kind_with_options(
                    &self.snapshot.options,
                    &old_program.snapshot.options,
                )
            };
            if pending_emit_kind != FileEmitKind::NONE {
                // Add all files to affectedFilesPending emit since emit changed
                for file in self.program.program().files() {
                    let path = source_path(file)?;
                    // Add to affectedFilesPending emit only if not changed since any changed file will do full emit
                    if !lock(&self.snapshot.changed_files_set).contains(&path) {
                        self.snapshot
                            .add_file_to_affected_files_pending_emit(&path, pending_emit_kind);
                    }
                }
                self.snapshot
                    .build_info_emit_pending
                    .store(true, Ordering::SeqCst);
            }
        }
        Ok(())
    }

    // port: tsc/internal/execute/incremental/programtosnapshot.go:toProgramSnapshot.handlePendingCheck
    fn handle_pending_check(&mut self) {
        if let Some(old_program) = self.old_program {
            if lock(&self.snapshot.semantic_diagnostics_per_file).len()
                != self.program.program().files().len()
                && old_program.snapshot.state().check_pending != self.snapshot.state().check_pending
            {
                self.snapshot
                    .build_info_emit_pending
                    .store(true, Ordering::SeqCst);
            }
        }
    }
}

/// `newReferences.Equals(oldReferences)`: both absent, or both present with
/// the same paths.
fn references_equal(new: Option<&ReferenceSet>, old: Option<&ReferenceSet>) -> bool {
    match (new, old) {
        (None, None) => true,
        (Some(new), Some(old)) => Arc::ptr_eq(new, old) || new == old,
        _ => false,
    }
}

// port: tsc/internal/execute/incremental/programtosnapshot.go:fileAffectsGlobalScope
pub(crate) fn file_affects_global_scope(file: &ProgramFile) -> Result<bool, Error> {
    // The program's files are bound when it is published (the pin's
    // `binder.BindSourceFile`).
    let view = file.bound().view();
    let ast = view.ast();
    let source = view.source_file()?;
    // if file contains anything that augments to global scope we need to build them as if
    // they are global files as well as module
    for augmentation in source.module_augmentations()?.iter().flatten() {
        if let Some(parent) = ast.node(*augmentation)?.parent() {
            if tsr_ast::utilities::is_global_scope_augmentation(&ast.node(parent)?) {
                return Ok(true);
            }
        }
    }

    if tsr_ast::utilities::is_external_or_common_js_module(&source)
        || tsr_ast::utilities::is_json_source_file(&source)
    {
        return Ok(false);
    }

    // For script files that contains only ambient external modules, although they are not actually external module files,
    // they can only be consumed via importing elements from them. Regular script files cannot consume them. Therefore,
    // there are no point to rebuild all script files if these special files have changed. However, if any statement
    // in the file is not ambient external module, we treat it as a regular script file.
    let statements = ast.node(file.source())?.statements(ast)?;
    for statement in ast.node_slice(statements)?.iter().flatten() {
        if !tsr_ast::utilities_modules::is_module_with_string_literal_name(ast, statement)? {
            return Ok(true);
        }
    }
    Ok(false)
}

// port: tsc/internal/execute/incremental/programtosnapshot.go:addReferencedFilesFromSymbol
fn add_referenced_files_from_symbol(
    program: &tsr_compiler::Program,
    file: &ProgramFile,
    referenced_files: &mut BTreeSet<Path>,
    checker: &Operation<'_>,
    symbol: Option<tsr_checker::SymbolRef>,
) -> Result<(), Error> {
    let Some(symbol) = symbol else {
        return Ok(());
    };
    for declaration in checker.symbol_declarations(symbol)?.iter().flatten() {
        let Some(file_of_decl) = program.file_of_node(declaration) else {
            continue;
        };
        if file.source() != file_of_decl.source() {
            referenced_files.insert(source_path(file_of_decl)?);
        }
    }
    Ok(())
}

/// Get the module source file and all augmenting files from the import name node from file
// port: tsc/internal/execute/incremental/programtosnapshot.go:addReferencedFilesFromImportLiteral
fn add_referenced_files_from_import_literal(
    program: &tsr_compiler::Program,
    file: &ProgramFile,
    referenced_files: &mut BTreeSet<Path>,
    checker: &mut Operation<'_>,
    import_name: tsr_ast::NodeId,
) -> Result<(), Error> {
    let symbol = checker.get_symbol_at_location(import_name)?;
    add_referenced_files_from_symbol(program, file, referenced_files, checker, symbol)
}

/// Gets the path to reference file from file name, it could be resolvedPath if present otherwise path
// port: tsc/internal/execute/incremental/programtosnapshot.go:addReferencedFileFromFileName
fn add_referenced_file_from_file_name(
    program: &tsr_compiler::Program,
    file_name: &[u8],
    referenced_files: &mut BTreeSet<Path>,
    source_file_directory: &[u8],
) {
    if let Some(redirect) = program.parse_file_redirect(file_name) {
        referenced_files.insert(tsr_tspath::to_path(
            redirect.as_bytes(),
            program.current_directory(),
            program.use_case_sensitive_file_names(),
        ));
    } else {
        referenced_files.insert(tsr_tspath::to_path(
            file_name,
            source_file_directory,
            program.use_case_sensitive_file_names(),
        ));
    }
}

/// Gets the referenced files for a file from the program with values for the keys as referenced file's path to be true
// port: tsc/internal/execute/incremental/programtosnapshot.go:getReferencedFiles
pub(crate) fn get_referenced_files(
    program: &CheckedProgram,
    file: &ProgramFile,
) -> Result<Option<ReferenceSet>, Error> {
    let loaded = program.program();
    let mut referenced_files = BTreeSet::new();

    // We need to use a set here since the code can contain the same import twice,
    // but that will only be one dependency.
    // To avoid invernal conversion, the key of the referencedFiles map must be of type Path
    let source = file.bound().view().source_file()?;
    let mut failure = None;
    program.with_type_checker_for_file_exclusive(
        &CheckerRequest::default(),
        file.source(),
        &mut |checker| {
            let result = (|| -> Result<(), Error> {
                for import_name in source.imports()?.iter().flatten() {
                    add_referenced_files_from_import_literal(
                        loaded,
                        file,
                        &mut referenced_files,
                        checker,
                        *import_name,
                    )?;
                }

                let source_file_directory = tsr_tspath::directory(source.file_name());
                // Handle triple slash references
                for referenced_file in source.referenced_files()?.iter() {
                    add_referenced_file_from_file_name(
                        loaded,
                        referenced_file.file_name.as_bytes(),
                        &mut referenced_files,
                        &source_file_directory,
                    );
                }

                // Handle type reference directives
                let path = source.parse_options().path.clone();
                for type_ref in loaded.resolved_type_reference_directives(path.as_bytes()) {
                    if !type_ref.result.resolved_file_name.is_empty() {
                        add_referenced_file_from_file_name(
                            loaded,
                            type_ref.result.resolved_file_name.as_bytes(),
                            &mut referenced_files,
                            &source_file_directory,
                        );
                    }
                }

                // Add module augmentation as references
                let ast = file.bound().view().ast();
                for module_name in source.module_augmentations()?.iter().flatten() {
                    if ast.node(*module_name)?.kind() != K::StringLiteral {
                        continue;
                    }
                    add_referenced_files_from_import_literal(
                        loaded,
                        file,
                        &mut referenced_files,
                        checker,
                        *module_name,
                    )?;
                }

                // From ambient modules
                for ambient_module in checker.get_ambient_modules()? {
                    add_referenced_files_from_symbol(
                        loaded,
                        file,
                        &mut referenced_files,
                        checker,
                        Some(ambient_module),
                    )?;
                }
                Ok(())
            })();
            if let Err(error) = result {
                failure = Some(error);
            }
            Ok(())
        },
    )?;
    if let Some(error) = failure {
        return Err(error);
    }
    Ok((!referenced_files.is_empty()).then(|| Arc::new(referenced_files)))
}

/// repopulateDiagnosticsOfFile repopulates diagnostic chains that depend on program state.
/// When diagnostics are copied from a previous build, their message chains may reference
/// stale program state (e.g., resolved module alternate results, package.json scope).
/// This function recomputes those chains using the current program's state.
// port: tsc/internal/execute/incremental/programtosnapshot.go:repopulateDiagnosticsOfFile
pub(crate) fn repopulate_diagnostics_of_file(
    diags: &Arc<DiagnosticsOrBuildInfoDiagnosticsWithFileName>,
    p: &Arc<tsr_compiler::Program>,
    file: &ProgramFile,
) -> Result<Arc<DiagnosticsOrBuildInfoDiagnosticsWithFileName>, Error> {
    if let Some(program_diagnostics) = diags.program_diagnostics() {
        let Some(repopulated) =
            repopulate_diagnostics_list(&program_diagnostics.diagnostics, p, file)?
        else {
            // Go's cached diagnostics retain only the source files they
            // reference. Our cache also carries its resolving program, so
            // sharing it unchanged would pin an entire obsolete program,
            // even for an empty list. Move its provenance to this program.
            if let Some(rebound) = program_diagnostics.rebind_for_reuse(p)? {
                return Ok(Arc::new(DiagnosticsOrBuildInfoDiagnosticsWithFileName {
                    diagnostics: Mutex::new(Some(rebound)),
                    ..DiagnosticsOrBuildInfoDiagnosticsWithFileName::default()
                }));
            }
            return Ok(diags.clone());
        };
        // The repopulated list is recorded as `p`'s, so the files it still
        // names as the old program held them must become `p`'s.
        let repopulated = ProgramDiagnostics {
            diagnostics: repopulated,
            ..program_diagnostics
        }
        .diagnostics_for(p)?;
        return Ok(Arc::new(
            DiagnosticsOrBuildInfoDiagnosticsWithFileName::from_diagnostics(p, repopulated),
        ));
    }
    // buildInfoDiagnostics will be repopulated via toDiagnostic's repopulateInfo handling
    Ok(diags.clone())
}

/// repopulateDiagnosticsList repopulates diagnostic chains in a list of diagnostics.
/// Returns `None` if no diagnostics needed repopulation (i.e., no changes were made).
// port: tsc/internal/execute/incremental/programtosnapshot.go:repopulateDiagnosticsList
fn repopulate_diagnostics_list(
    diags: &[Diagnostic],
    p: &Arc<tsr_compiler::Program>,
    file: &ProgramFile,
) -> Result<Option<Vec<Diagnostic>>, Error> {
    let mut changed = false;
    let mut result = Vec::with_capacity(diags.len());
    for d in diags {
        let repopulated = repopulate_diagnostic_message_chain(&d.message_chain, p, file)?;
        if let Some(repopulated) = repopulated {
            let mut clone = d.clone();
            clone.message_chain = repopulated;
            result.push(clone);
            changed = true;
        } else {
            result.push(d.clone());
        }
    }
    if !changed {
        return Ok(None);
    }
    Ok(Some(result))
}

/// repopulateDiagnosticMessageChain repopulates chains that have repopulate info.
/// Returns `None` if no changes were made.
// port: tsc/internal/execute/incremental/programtosnapshot.go:repopulateDiagnosticMessageChain
fn repopulate_diagnostic_message_chain(
    chain: &[Arc<Diagnostic>],
    p: &Arc<tsr_compiler::Program>,
    file: &ProgramFile,
) -> Result<Option<Vec<Arc<Diagnostic>>>, Error> {
    if chain.is_empty() {
        return Ok(None);
    }
    let mut changed = false;
    let mut result = Vec::with_capacity(chain.len());
    for c in chain {
        if c.repopulate_info.is_some() {
            // Convert to buildInfoDiagnosticWithFileName and repopulate
            let mut b = BuildInfoDiagnosticWithFileName {
                pos: c.loc.pos(),
                end: c.loc.end(),
                code: c.code,
                category: c.category,
                source: c.source.clone(),
                message_text: c.message_text.clone(),
                message_key: c.message_key.clone(),
                message_args: c.message_args.clone(),
                repopulate_info: c.repopulate_info.clone(),
                ..BuildInfoDiagnosticWithFileName::default()
            };
            // Recursively handle nested chains
            for nested in &c.message_chain {
                b.message_chain.push(ast_diag_to_build_info_diag(nested));
            }
            result.push(Arc::new(crate::snapshot::repopulate_diagnostic_chain(
                &b,
                p,
                Some(file.source()),
            )?));
            changed = true;
        } else {
            // Check nested chains
            let nested = repopulate_diagnostic_message_chain(&c.message_chain, p, file)?;
            if let Some(nested) = nested {
                let mut clone = Diagnostic::clone(c);
                clone.message_chain = nested;
                result.push(Arc::new(clone));
                changed = true;
            } else {
                result.push(c.clone());
            }
        }
    }
    if !changed {
        return Ok(None);
    }
    Ok(Some(result))
}

// port: tsc/internal/execute/incremental/programtosnapshot.go:astDiagToBuildInfoDiag
pub(crate) fn ast_diag_to_build_info_diag(d: &Diagnostic) -> BuildInfoDiagnosticWithFileName {
    let mut b = BuildInfoDiagnosticWithFileName {
        pos: d.loc.pos(),
        end: d.loc.end(),
        code: d.code,
        category: d.category,
        source: d.source.clone(),
        message_text: d.message_text.clone(),
        message_key: d.message_key.clone(),
        message_args: d.message_args.clone(),
        repopulate_info: d.repopulate_info.clone(),
        ..BuildInfoDiagnosticWithFileName::default()
    };
    for nested in &d.message_chain {
        b.message_chain.push(ast_diag_to_build_info_diag(nested));
    }
    b
}
