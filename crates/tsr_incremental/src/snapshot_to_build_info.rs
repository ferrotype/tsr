//! The build info of a program's incremental state
//! (`snapshottobuildinfo.go`): file names become 1-based ids, paths become
//! relative to the build info's directory, and the reference sets, the
//! diagnostics and the pending emit are written in path order.
use crate::build_info::{
    content_mapper_identities, new_build_info_file_info, BuildInfo, BuildInfoDiagnostic,
    BuildInfoDiagnosticsOfFile, BuildInfoEmitSignature, BuildInfoFileId, BuildInfoFileIdListId,
    BuildInfoFilePendingEmit, BuildInfoReferenceMapEntry, BuildInfoRepopulateInfo,
    BuildInfoResolvedRoot, BuildInfoRoot, BuildInfoSemanticDiagnostic,
};
use crate::json::AnyValue;
use crate::program::source_path;
use crate::reference_map::ReferenceSet;
use crate::snapshot::{
    file_path, get_file_emit_kind, lock, BuildInfoDiagnosticWithFileName,
    DiagnosticsOrBuildInfoDiagnosticsWithFileName, FileEmitKind, Path, Snapshot,
};
use std::collections::HashMap;
use std::sync::Arc;
use tsr_ast::diagnostic_api::RepopulateDiagnosticInfo;
use tsr_ast::{Diagnostic, NodeId};
use tsr_compiler::{CheckedProgram, Error, Program, ProgramFile};
use tsr_core::collections::OrderedMap;
use tsr_core::ModuleKind;
use tsr_jsstring::JsString;
use tsr_tsoptions::affects::OptionField;
use tsr_tsoptions::{ConfigValue, OptionKind};

/// The build info, or the content-mapper project's error (the pin's
/// `error` result); the outer error is a compiler failure.
// port: tsc/internal/execute/incremental/snapshottobuildinfo.go:snapshotToBuildInfo
pub(crate) fn snapshot_to_build_info(
    snapshot: &Snapshot,
    program: &CheckedProgram,
    build_info_file_name: &JsString,
) -> Result<Result<BuildInfo, tsr_contentmapper::Error>, Error> {
    let loaded = program.program();
    let content_mapper_identities =
        match content_mapper_identities(loaded.content_mapper_project().map(AsRef::as_ref)) {
            Ok(identities) => identities,
            Err(error) => return Ok(Err(error)),
        };
    let build_info = BuildInfo {
        version: JsString::from_bytes(tsr_core::version().as_bytes()),
        content_mapper_identities,
        ..BuildInfo::default()
    };
    let mut to = ToBuildInfo {
        snapshot,
        program,
        build_info,
        build_info_directory: tsr_tspath::directory(build_info_file_name.as_bytes()),
        compare_paths_options_current_directory: loaded.current_directory().to_vec(),
        compare_paths_options_use_case_sensitive_file_names: loaded.use_case_sensitive_file_names(),
        file_name_to_file_id: HashMap::new(),
        file_names_to_file_id_list_id: HashMap::new(),
        roots: HashMap::new(),
    };

    if snapshot.options.is_incremental() {
        to.collect_root_files()?;
        to.set_file_info_and_emit_signatures()?;
        to.set_root_of_incremental_program()?;
        to.set_compiler_options();
        to.set_referenced_map();
        to.set_change_file_set();
        to.set_semantic_diagnostics()?;
        to.set_emit_diagnostics()?;
        to.set_affected_files_pending_emit()?;
        let latest_changed_dts_file = snapshot.state().latest_changed_dts_file.clone();
        if !latest_changed_dts_file.is_empty() {
            to.build_info.latest_changed_dts_file =
                to.relative_to_build_info(latest_changed_dts_file.as_bytes());
        }
    } else {
        to.set_root_of_non_incremental_program();
    }
    let state = snapshot.state().clone();
    to.build_info.errors = state.has_errors.is_true();
    to.build_info.semantic_errors = state.has_semantic_errors;
    to.build_info.check_pending = state.check_pending;
    to.set_package_jsons();
    Ok(Ok(to.build_info))
}

struct ToBuildInfo<'a> {
    snapshot: &'a Snapshot,
    program: &'a CheckedProgram,
    build_info: BuildInfo,
    build_info_directory: Vec<u8>,
    compare_paths_options_current_directory: Vec<u8>,
    compare_paths_options_use_case_sensitive_file_names: bool,
    file_name_to_file_id: HashMap<Path, BuildInfoFileId>,
    file_names_to_file_id_list_id: HashMap<Vec<u8>, BuildInfoFileIdListId>,
    /// Each root file with the path of the root name that included it (the
    /// pin's map from file to root path).
    roots: HashMap<NodeId, (Arc<ProgramFile>, Path)>,
}

#[allow(clippy::wrong_self_convention)] // the pin's `toBuildInfo` method names
impl ToBuildInfo<'_> {
    fn loaded(&self) -> &Arc<Program> {
        self.program.program()
    }

    // port: tsc/internal/execute/incremental/snapshottobuildinfo.go:toBuildInfo.relativeToBuildInfo
    fn relative_to_build_info(&self, path: &[u8]) -> JsString {
        JsString::from_bytes(
            tsr_tspath::ensure_path_is_non_module_name(&tsr_tspath::relative_from_directory(
                &self.build_info_directory,
                path,
                &self.compare_paths_options_current_directory,
                self.compare_paths_options_use_case_sensitive_file_names,
            ))
            .into_owned(),
        )
    }

    // port: tsc/internal/execute/incremental/snapshottobuildinfo.go:toBuildInfo.toFileId
    fn to_file_id(&mut self, path: &Path) -> BuildInfoFileId {
        let mut file_id = self.file_name_to_file_id.get(path).copied().unwrap_or(0);
        if file_id == 0 {
            let lib_file = self.loaded().default_lib_file(path.as_bytes()).cloned();
            if let Some(lib_file) = lib_file.filter(|lib_file| !lib_file.replaced) {
                self.build_info.file_names.push(lib_file.name);
            } else {
                let relative = self.relative_to_build_info(path.as_bytes());
                self.build_info.file_names.push(relative);
            }
            file_id = self.build_info.file_names.len() as BuildInfoFileId;
            self.file_name_to_file_id.insert(path.clone(), file_id);
        }
        file_id
    }

    // port: tsc/internal/execute/incremental/snapshottobuildinfo.go:toBuildInfo.toFileIdListId
    fn to_file_id_list_id(&mut self, set: &ReferenceSet) -> BuildInfoFileIdListId {
        let mut file_ids: Vec<BuildInfoFileId> =
            set.iter().map(|path| self.to_file_id(path)).collect();
        file_ids.sort_unstable();
        let key = file_ids
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",")
            .into_bytes();

        let mut file_id_list_id = self
            .file_names_to_file_id_list_id
            .get(&key)
            .copied()
            .unwrap_or(0);
        if file_id_list_id == 0 {
            self.build_info.file_ids_list.push(file_ids);
            file_id_list_id = self.build_info.file_ids_list.len() as BuildInfoFileIdListId;
            self.file_names_to_file_id_list_id
                .insert(key, file_id_list_id);
        }
        file_id_list_id
    }

    // port: tsc/internal/execute/incremental/snapshottobuildinfo.go:toBuildInfo.toRelativeToBuildInfoCompilerOptionValue
    fn to_relative_to_build_info_compiler_option_value(
        &self,
        option: &tsr_tsoptions::OptionDeclaration,
        v: &ConfigValue,
    ) -> ConfigValue {
        if option.kind == OptionKind::List {
            if option.element.is_some_and(|element| element.is_file_path) {
                if let ConfigValue::StringArray(Some(arr)) = v {
                    return ConfigValue::StringArray(Some(
                        arr.iter()
                            .map(|value| self.relative_to_build_info(value.as_bytes()))
                            .collect(),
                    ));
                }
                if let ConfigValue::Array(Some(arr)) = v {
                    if arr
                        .iter()
                        .all(|value| matches!(value, ConfigValue::String(_)))
                    {
                        return ConfigValue::Array(Some(
                            arr.iter()
                                .map(|value| {
                                    let ConfigValue::String(value) = value else {
                                        unreachable!("checked string elements")
                                    };
                                    ConfigValue::String(
                                        self.relative_to_build_info(value.as_bytes()),
                                    )
                                })
                                .collect(),
                        ));
                    }
                }
            }
        } else if option.is_file_path {
            if let ConfigValue::String(str) = v {
                if !str.is_empty() {
                    return ConfigValue::String(self.relative_to_build_info(str.as_bytes()));
                }
            }
        }
        v.clone()
    }

    // port: tsc/internal/execute/incremental/snapshottobuildinfo.go:toBuildInfo.toBuildInfoDiagnosticsFromFileNameDiagnostics
    fn to_build_info_diagnostics_from_file_name_diagnostics(
        &mut self,
        diagnostics: &[BuildInfoDiagnosticWithFileName],
    ) -> Vec<BuildInfoDiagnostic> {
        diagnostics
            .iter()
            .map(|d| {
                let mut file = 0;
                if !d.file.is_empty() {
                    file = self.to_file_id(&d.file);
                }
                BuildInfoDiagnostic {
                    file,
                    no_file: d.no_file,
                    pos: d.pos,
                    end: d.end,
                    code: d.code,
                    category: d.category,
                    source: d.source.clone(),
                    message_text: d.message_text.clone(),
                    message_key: d.message_key.clone(),
                    message_args: d.message_args.clone(),
                    message_chain: self
                        .to_build_info_diagnostics_from_file_name_diagnostics(&d.message_chain),
                    related_information: self.to_build_info_diagnostics_from_file_name_diagnostics(
                        &d.related_information,
                    ),
                    reports_unnecessary: d.reports_unnecessary,
                    reports_deprecated: d.reports_deprecated,
                    skipped_on_no_emit: d.skipped_on_no_emit,
                    repopulate_info: to_build_info_repopulate_info(d.repopulate_info.as_deref()),
                }
            })
            .collect()
    }

    // port: tsc/internal/execute/incremental/snapshottobuildinfo.go:toBuildInfo.toBuildInfoDiagnosticsFromDiagnostics
    fn to_build_info_diagnostics_from_diagnostics(
        &mut self,
        owner: &Program,
        file_path_of_diagnostics: &Path,
        diagnostics: &[impl std::borrow::Borrow<Diagnostic>],
    ) -> Result<Vec<BuildInfoDiagnostic>, Error> {
        let mut result = Vec::with_capacity(diagnostics.len());
        for d in diagnostics {
            let d: &Diagnostic = d.borrow();
            let mut file = 0;
            let mut no_file = false;
            match d.file {
                None => no_file = true,
                Some(diagnostic_file) => {
                    let diagnostic_path = file_path(owner, diagnostic_file)?;
                    if diagnostic_path != *file_path_of_diagnostics {
                        file = self.to_file_id(&diagnostic_path);
                    }
                }
            }
            result.push(BuildInfoDiagnostic {
                file,
                no_file,
                pos: d.loc.pos(),
                end: d.loc.end(),
                code: d.code,
                category: d.category,
                source: d.source.clone(),
                message_text: d.message_text.clone(),
                message_key: d.message_key.clone(),
                message_args: d.message_args.clone(),
                message_chain: self.to_build_info_diagnostics_from_diagnostics(
                    owner,
                    file_path_of_diagnostics,
                    &d.message_chain,
                )?,
                related_information: self.to_build_info_diagnostics_from_diagnostics(
                    owner,
                    file_path_of_diagnostics,
                    &d.related_information,
                )?,
                reports_unnecessary: d.reports_unnecessary,
                reports_deprecated: d.reports_deprecated,
                skipped_on_no_emit: d.skipped_on_no_emit,
                repopulate_info: to_build_info_repopulate_info(d.repopulate_info.as_deref()),
            });
        }
        Ok(result)
    }

    // port: tsc/internal/execute/incremental/snapshottobuildinfo.go:toBuildInfo.toBuildInfoDiagnosticsOfFile
    fn to_build_info_diagnostics_of_file(
        &mut self,
        file_path: &Path,
        diags: &DiagnosticsOrBuildInfoDiagnosticsWithFileName,
    ) -> Result<Option<BuildInfoDiagnosticsOfFile>, Error> {
        if let Some(program_diagnostics) = diags
            .program_diagnostics()
            .filter(|diagnostics| !diagnostics.diagnostics.is_empty())
        {
            return Ok(Some(BuildInfoDiagnosticsOfFile {
                file_id: self.to_file_id(file_path),
                diagnostics: self.to_build_info_diagnostics_from_diagnostics(
                    &program_diagnostics.program,
                    file_path,
                    &program_diagnostics.diagnostics,
                )?,
            }));
        }
        if !diags.build_info_diagnostics.is_empty() {
            return Ok(Some(BuildInfoDiagnosticsOfFile {
                file_id: self.to_file_id(file_path),
                diagnostics: self.to_build_info_diagnostics_from_file_name_diagnostics(
                    &diags.build_info_diagnostics,
                ),
            }));
        }
        Ok(None)
    }

    // port: tsc/internal/execute/incremental/snapshottobuildinfo.go:toBuildInfo.collectRootFiles
    fn collect_root_files(&mut self) -> Result<(), Error> {
        let loaded = self.loaded().clone();
        for file_name in &loaded.config().root_file_names {
            let file = if let Some(redirect) = loaded.parse_file_redirect(file_name.as_bytes()) {
                loaded.source_file(redirect.as_bytes())
            } else {
                loaded.source_file(file_name.as_bytes())
            };
            if let Some(file) = file {
                let file = crate::program::file_arc(self.program, file)?;
                let root = tsr_tspath::to_path(
                    file_name.as_bytes(),
                    &self.compare_paths_options_current_directory,
                    self.compare_paths_options_use_case_sensitive_file_names,
                );
                // The pin's map keeps the last root name that included the file.
                self.roots.insert(file.source(), (file, root));
            }
        }
        Ok(())
    }

    // port: tsc/internal/execute/incremental/snapshottobuildinfo.go:toBuildInfo.setFileInfoAndEmitSignatures
    fn set_file_info_and_emit_signatures(&mut self) -> Result<(), Error> {
        let loaded = self.loaded().clone();
        let mut file_infos = Vec::with_capacity(loaded.files().len());
        for file in loaded.files() {
            let path = source_path(file)?;
            let info = lock(&self.snapshot.file_infos)
                .get(&path)
                .cloned()
                .expect("every program file has its file info");
            let file_id = self.to_file_id(&path);
            //  tryAddRoot(key, fileId);
            let file_name = self.build_info.file_names[(file_id - 1) as usize].clone();
            if file_name != self.relative_to_build_info(path.as_bytes()) {
                let lib_file = loaded.default_lib_file(path.as_bytes());
                assert!(
                    lib_file.is_some_and(|lib_file| !lib_file.replaced && file_name == lib_file.name),
                    "File name at index {} does not match expected relative path or libName: {} != {}",
                    file_id - 1,
                    String::from_utf8_lossy(file_name.as_bytes()),
                    String::from_utf8_lossy(self.relative_to_build_info(path.as_bytes()).as_bytes())
                );
            }
            if self.snapshot.options.composite.is_true() {
                let source = file.bound().view().source_file()?;
                if !tsr_ast::utilities::is_json_source_file(&source)
                    && loaded.source_file_may_be_emitted(file, false)?
                {
                    let emit_signature = lock(&self.snapshot.emit_signatures).get(&path).cloned();
                    match emit_signature {
                        None => {
                            self.build_info
                                .emit_signatures
                                .push(BuildInfoEmitSignature {
                                    file_id,
                                    ..BuildInfoEmitSignature::default()
                                });
                        }
                        Some(emit_signature) if emit_signature.signature != info.signature => {
                            let mut incremental_emit_signature = BuildInfoEmitSignature {
                                file_id,
                                ..BuildInfoEmitSignature::default()
                            };
                            let different = emit_signature
                                .signature_with_different_options
                                .as_deref()
                                .unwrap_or_default();
                            if !emit_signature.signature.is_empty() {
                                incremental_emit_signature
                                    .signature
                                    .clone_from(&emit_signature.signature);
                            } else if different[0] == info.signature {
                                incremental_emit_signature.differs_only_in_dts_map = true;
                            } else {
                                incremental_emit_signature.signature = different[0].clone();
                                incremental_emit_signature.differs_in_options = true;
                            }
                            self.build_info
                                .emit_signatures
                                .push(incremental_emit_signature);
                        }
                        Some(_) => {}
                    }
                }
            }
            file_infos.push(new_build_info_file_info(&info));
        }
        self.build_info.file_infos = Some(file_infos);
        Ok(())
    }

    // port: tsc/internal/execute/incremental/snapshottobuildinfo.go:toBuildInfo.setRootOfIncrementalProgram
    fn set_root_of_incremental_program(&mut self) -> Result<(), Error> {
        let mut keys = Vec::with_capacity(self.roots.len());
        for (file, root) in std::mem::take(&mut self.roots).into_values() {
            let path = source_path(&file)?;
            let file_id = self.to_file_id(&path);
            keys.push((file_id, path, root));
        }
        keys.sort_by_key(|(file_id, _, _)| *file_id);
        for (_, path, root_path) in keys {
            let root = self.to_file_id(&root_path);
            let resolved = self.to_file_id(&path);
            let roots = self.build_info.root.get_or_insert_with(Vec::new);
            if roots.is_empty() {
                // First fileId as is
                roots.push(BuildInfoRoot {
                    start: resolved,
                    ..BuildInfoRoot::default()
                });
            } else {
                let last = roots.last_mut().expect("a root");
                if last.end == resolved - 1 {
                    // If its [..., last = [start, end = fileId - 1]], update last to [start, fileId]
                    last.end = resolved;
                } else if last.end == 0 && last.start == resolved - 1 {
                    // If its [..., last = start = fileId - 1 ], update last to [start, fileId]
                    last.end = resolved;
                } else {
                    roots.push(BuildInfoRoot {
                        start: resolved,
                        ..BuildInfoRoot::default()
                    });
                }
            }
            if root != resolved {
                self.build_info
                    .resolved_root
                    .push(BuildInfoResolvedRoot { resolved, root });
            }
        }
        Ok(())
    }

    // port: tsc/internal/execute/incremental/snapshottobuildinfo.go:toBuildInfo.setCompilerOptions
    fn set_compiler_options(&mut self) {
        let mut options: Option<OrderedMap<JsString, AnyValue>> = None;
        tsr_tsoptions::affects::for_each_compiler_option_value(
            &self.snapshot.options,
            |field: &OptionField| tsr_tsoptions::affects::affects_build_info(field.declaration),
            &mut |field, value, _| {
                let Some(value) = value else {
                    return false;
                };
                let option = tsr_tsoptions::option_declaration(field.declaration.as_bytes(), false)
                    .expect("an option field's declaration");
                // Make it relative to buildInfo directory if file path
                options.get_or_insert_with(OrderedMap::default).insert(
                    JsString::from_bytes(option.name.as_bytes()),
                    AnyValue(self.to_relative_to_build_info_compiler_option_value(option, value)),
                );
                false
            },
        );
        self.build_info.options = options;
    }

    // port: tsc/internal/execute/incremental/snapshottobuildinfo.go:toBuildInfo.setReferencedMap
    fn set_referenced_map(&mut self) {
        let mut keys = self.snapshot.referenced_map.get_paths_with_references();
        keys.sort();
        self.build_info.referenced_map = keys
            .iter()
            .map(|file_path| {
                let references = self
                    .snapshot
                    .referenced_map
                    .get_references(file_path)
                    .expect("a path with references");
                BuildInfoReferenceMapEntry {
                    file_id: self.to_file_id(file_path),
                    file_id_list_id: self.to_file_id_list_id(&references),
                }
            })
            .collect();
    }

    // port: tsc/internal/execute/incremental/snapshottobuildinfo.go:toBuildInfo.setChangeFileSet
    fn set_change_file_set(&mut self) {
        let mut files: Vec<Path> = lock(&self.snapshot.changed_files_set)
            .iter()
            .cloned()
            .collect();
        files.sort();
        self.build_info.change_file_set = files.iter().map(|path| self.to_file_id(path)).collect();
    }

    // port: tsc/internal/execute/incremental/snapshottobuildinfo.go:toBuildInfo.setSemanticDiagnostics
    fn set_semantic_diagnostics(&mut self) -> Result<(), Error> {
        let loaded = self.loaded().clone();
        for file in loaded.files() {
            let path = source_path(file)?;
            let value = lock(&self.snapshot.semantic_diagnostics_per_file)
                .get(&path)
                .cloned();
            match value {
                None => {
                    if !lock(&self.snapshot.changed_files_set).contains(&path) {
                        let file_id = self.to_file_id(&path);
                        self.build_info.semantic_diagnostics_per_file.push(
                            BuildInfoSemanticDiagnostic {
                                file_id,
                                diagnostics: None,
                            },
                        );
                    }
                }
                Some(value) => {
                    let diagnostics = self.to_build_info_diagnostics_of_file(&path, &value)?;
                    if let Some(diagnostics) = diagnostics {
                        self.build_info.semantic_diagnostics_per_file.push(
                            BuildInfoSemanticDiagnostic {
                                file_id: 0,
                                diagnostics: Some(diagnostics),
                            },
                        );
                    }
                }
            }
        }
        Ok(())
    }

    // port: tsc/internal/execute/incremental/snapshottobuildinfo.go:toBuildInfo.setEmitDiagnostics
    fn set_emit_diagnostics(&mut self) -> Result<(), Error> {
        let mut files: Vec<(Path, Arc<DiagnosticsOrBuildInfoDiagnosticsWithFileName>)> =
            lock(&self.snapshot.emit_diagnostics_per_file)
                .iter()
                .map(|(path, value)| (path.clone(), value.clone()))
                .collect();
        files.sort_by(|a, b| a.0.cmp(&b.0));
        let mut emit_diagnostics_per_file = Vec::with_capacity(files.len());
        for (file_path, value) in files {
            emit_diagnostics_per_file
                .push(self.to_build_info_diagnostics_of_file(&file_path, &value)?);
        }
        self.build_info.emit_diagnostics_per_file = emit_diagnostics_per_file;
        Ok(())
    }

    // port: tsc/internal/execute/incremental/snapshottobuildinfo.go:toBuildInfo.setAffectedFilesPendingEmit
    fn set_affected_files_pending_emit(&mut self) -> Result<(), Error> {
        let pending: Vec<(Path, FileEmitKind)> = lock(&self.snapshot.affected_files_pending_emit)
            .iter()
            .map(|(path, kind)| (path.clone(), *kind))
            .collect();
        let full_emit_kind = get_file_emit_kind(&self.snapshot.options);
        let loaded = self.loaded().clone();
        for (file_path, pending_emit) in pending {
            let Some(file) = loaded.file(file_path.as_bytes()) else {
                continue;
            };
            if !loaded.source_file_may_be_emitted(file, false)? {
                continue;
            }
            let file_id = self.to_file_id(&file_path);
            self.build_info
                .affected_files_pending_emit
                .push(BuildInfoFilePendingEmit {
                    file_id,
                    emit_kind: if pending_emit == full_emit_kind {
                        FileEmitKind::NONE
                    } else {
                        pending_emit
                    },
                });
        }
        Ok(())
    }

    // port: tsc/internal/execute/incremental/snapshottobuildinfo.go:toBuildInfo.setRootOfNonIncrementalProgram
    fn set_root_of_non_incremental_program(&mut self) {
        let loaded = self.loaded().clone();
        self.build_info.root = Some(
            loaded
                .config()
                .root_file_names
                .iter()
                .map(|file_name| BuildInfoRoot {
                    non_incremental: self.relative_to_build_info(
                        tsr_tspath::to_path(
                            file_name.as_bytes(),
                            &self.compare_paths_options_current_directory,
                            self.compare_paths_options_use_case_sensitive_file_names,
                        )
                        .as_bytes(),
                    ),
                    ..BuildInfoRoot::default()
                })
                .collect(),
        );
    }

    // port: tsc/internal/execute/incremental/snapshottobuildinfo.go:toBuildInfo.setPackageJsons
    fn set_package_jsons(&mut self) {
        let state = self.snapshot.state().clone();
        if let Some(package_jsons) = state.package_jsons.filter(|jsons| !jsons.is_empty()) {
            self.build_info.package_jsons = package_jsons
                .iter()
                .map(|path| self.relative_to_build_info(path.as_bytes()))
                .collect();
        }
        if let Some(missing_package_jsons) = state
            .missing_package_jsons
            .filter(|jsons| !jsons.is_empty())
        {
            self.build_info.missing_package_jsons = missing_package_jsons
                .iter()
                .map(|path| self.relative_to_build_info(path.as_bytes()))
                .collect();
        }
    }
}

// port: tsc/internal/execute/incremental/snapshottobuildinfo.go:toBuildInfoRepopulateInfo
fn to_build_info_repopulate_info(
    info: Option<&RepopulateDiagnosticInfo>,
) -> Option<BuildInfoRepopulateInfo> {
    let info = info?;
    Some(BuildInfoRepopulateInfo {
        kind: info.kind,
        module_reference: info.module_reference.clone(),
        mode: ModuleKind(info.mode),
        package_name: info.package_name.clone(),
    })
}
