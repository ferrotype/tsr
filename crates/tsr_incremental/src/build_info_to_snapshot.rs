//! A build info read back as the incremental state of the old program
//! (`buildinfotosnapshot.go`).
use crate::build_info::{
    is_build_info_file_name_default_library, BuildInfo, BuildInfoDiagnostic,
    BuildInfoDiagnosticsOfFile, BuildInfoFileId, BuildInfoFileIdListId, BuildInfoFileInfo,
    BuildInfoRepopulateInfo,
};
use crate::host::CompilerHost;
use crate::reference_map::ReferenceSet;
use crate::snapshot::{
    get_file_emit_kind, lock, BuildInfoDiagnosticWithFileName,
    DiagnosticsOrBuildInfoDiagnosticsWithFileName, EmitSignature, FileEmitKind, Path, Snapshot,
};
use std::collections::BTreeSet;
use std::sync::Arc;
use tsr_ast::diagnostic_api::RepopulateDiagnosticInfo;
use tsr_core::Tristate;
use tsr_jsstring::JsString;
use tsr_tsoptions::ParsedCommandLine;

// port: tsc/internal/execute/incremental/buildinfotosnapshot.go:buildInfoToSnapshot
pub(crate) fn build_info_to_snapshot(
    build_info: &BuildInfo,
    config: &ParsedCommandLine,
    host: &dyn CompilerHost,
) -> Snapshot {
    let mut to = ToSnapshot {
        build_info,
        build_info_directory: tsr_tspath::directory(&tsr_tspath::absolute(
            config.build_info_file_name().as_bytes(),
            config.current_directory(),
        )),
        snapshot: Snapshot::default(),
        file_paths: Vec::with_capacity(build_info.file_names.len()),
        file_path_set: Vec::with_capacity(build_info.file_ids_list.len()),
    };
    to.file_paths = build_info
        .file_names
        .iter()
        .map(|file_name| {
            if is_build_info_file_name_default_library(file_name.as_bytes()) {
                return tsr_tspath::to_path(
                    &tsr_tspath::combine(host.default_library_path(), &[file_name.as_bytes()]),
                    host.get_current_directory(),
                    host.fs().use_case_sensitive_file_names(),
                );
            }
            tsr_tspath::to_path(
                file_name.as_bytes(),
                &to.build_info_directory,
                config.use_case_sensitive_file_names(),
            )
        })
        .collect();
    to.file_path_set = build_info
        .file_ids_list
        .iter()
        .map(|file_id_list| {
            let mut file_set = BTreeSet::new();
            for &file_id in file_id_list {
                file_set.insert(to.to_file_path(file_id));
            }
            Arc::new(file_set)
        })
        .collect();
    to.set_compiler_options();
    to.set_file_info_and_emit_signatures();
    to.set_referenced_map();
    to.set_change_file_set();
    to.set_semantic_diagnostics();
    to.set_emit_diagnostics();
    to.set_affected_files_pending_emit();
    if !build_info.latest_changed_dts_file.is_empty() {
        let latest = to.to_absolute_path(&build_info.latest_changed_dts_file);
        to.snapshot.state().latest_changed_dts_file = latest;
    }
    {
        let mut state = to.snapshot.state();
        state.has_errors = if build_info.errors {
            Tristate::TRUE
        } else {
            Tristate::FALSE
        };
        state.has_semantic_errors = build_info.semantic_errors;
        state.check_pending = build_info.check_pending;
    }
    to.set_package_jsons();
    to.snapshot
}

struct ToSnapshot<'a> {
    build_info: &'a BuildInfo,
    build_info_directory: Vec<u8>,
    snapshot: Snapshot,
    file_paths: Vec<Path>,
    file_path_set: Vec<ReferenceSet>,
}

impl ToSnapshot<'_> {
    // port: tsc/internal/execute/incremental/buildinfotosnapshot.go:toSnapshot.toAbsolutePath
    fn to_absolute_path(&self, path: &JsString) -> JsString {
        JsString::from_bytes(tsr_tspath::absolute(
            path.as_bytes(),
            &self.build_info_directory,
        ))
    }

    // port: tsc/internal/execute/incremental/buildinfotosnapshot.go:toSnapshot.toFilePath
    fn to_file_path(&self, file_id: BuildInfoFileId) -> Path {
        self.file_paths[(file_id - 1) as usize].clone()
    }

    // port: tsc/internal/execute/incremental/buildinfotosnapshot.go:toSnapshot.toFilePathSet
    fn to_file_path_set(&self, file_id_list_id: BuildInfoFileIdListId) -> ReferenceSet {
        self.file_path_set[(file_id_list_id - 1) as usize].clone()
    }

    // port: tsc/internal/execute/incremental/buildinfotosnapshot.go:toSnapshot.toBuildInfoDiagnosticsWithFileName
    fn to_build_info_diagnostics_with_file_name(
        &self,
        diagnostics: &[BuildInfoDiagnostic],
    ) -> Vec<BuildInfoDiagnosticWithFileName> {
        diagnostics
            .iter()
            .map(|d| {
                let mut file = Path::default();
                if d.file != 0 {
                    file = self.to_file_path(d.file);
                }
                BuildInfoDiagnosticWithFileName {
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
                    message_chain: self.to_build_info_diagnostics_with_file_name(&d.message_chain),
                    related_information: self
                        .to_build_info_diagnostics_with_file_name(&d.related_information),
                    reports_unnecessary: d.reports_unnecessary,
                    reports_deprecated: d.reports_deprecated,
                    skipped_on_no_emit: d.skipped_on_no_emit,
                    repopulate_info: from_build_info_repopulate_info(d.repopulate_info.as_ref()),
                }
            })
            .collect()
    }

    // port: tsc/internal/execute/incremental/buildinfotosnapshot.go:toSnapshot.toDiagnosticsOrBuildInfoDiagnosticsWithFileName
    fn to_diagnostics_or_build_info_diagnostics_with_file_name(
        &self,
        dig: &BuildInfoDiagnosticsOfFile,
    ) -> DiagnosticsOrBuildInfoDiagnosticsWithFileName {
        DiagnosticsOrBuildInfoDiagnosticsWithFileName {
            build_info_diagnostics: self.to_build_info_diagnostics_with_file_name(&dig.diagnostics),
            ..DiagnosticsOrBuildInfoDiagnosticsWithFileName::default()
        }
    }

    // port: tsc/internal/execute/incremental/buildinfotosnapshot.go:toSnapshot.setCompilerOptions
    fn set_compiler_options(&mut self) {
        self.snapshot.options = Arc::new(
            self.build_info
                .get_compiler_options(&self.build_info_directory),
        );
    }

    // port: tsc/internal/execute/incremental/buildinfotosnapshot.go:toSnapshot.setFileInfoAndEmitSignatures
    fn set_file_info_and_emit_signatures(&mut self) {
        let is_composite = self.snapshot.options.composite.is_true();
        for (index, build_info_file_info) in self.build_info.file_infos.iter().flatten().enumerate()
        {
            let path = self.to_file_path(index as BuildInfoFileId + 1);
            let info =
                BuildInfoFileInfo::get_file_info(Some(build_info_file_info)).expect("a file info");
            let signature = info.signature.clone();
            lock(&self.snapshot.file_infos).insert(path.clone(), info);
            // Add default emit signature as file's signature
            if !signature.is_empty() && is_composite {
                lock(&self.snapshot.emit_signatures).insert(
                    path,
                    EmitSignature {
                        signature,
                        signature_with_different_options: None,
                    },
                );
            }
        }
        // Fix up emit signatures
        for value in &self.build_info.emit_signatures {
            if value.no_emit_signature() {
                lock(&self.snapshot.emit_signatures).remove(&self.to_file_path(value.file_id));
            } else {
                let path = self.to_file_path(value.file_id);
                let signature = value.to_emit_signature(&path, &self.snapshot.emit_signatures);
                lock(&self.snapshot.emit_signatures).insert(path, signature);
            }
        }
    }

    // port: tsc/internal/execute/incremental/buildinfotosnapshot.go:toSnapshot.setReferencedMap
    fn set_referenced_map(&mut self) {
        for entry in &self.build_info.referenced_map {
            self.snapshot.referenced_map.store_references(
                &self.to_file_path(entry.file_id),
                self.to_file_path_set(entry.file_id_list_id),
            );
        }
    }

    // port: tsc/internal/execute/incremental/buildinfotosnapshot.go:toSnapshot.setChangeFileSet
    fn set_change_file_set(&mut self) {
        for &file_id in &self.build_info.change_file_set {
            let file_path = self.to_file_path(file_id);
            lock(&self.snapshot.changed_files_set).insert(file_path);
        }
    }

    // port: tsc/internal/execute/incremental/buildinfotosnapshot.go:toSnapshot.setSemanticDiagnostics
    fn set_semantic_diagnostics(&mut self) {
        let paths: Vec<Path> = lock(&self.snapshot.file_infos).keys().cloned().collect();
        for path in paths {
            // Initialize to have no diagnostics if its not changed file
            if !lock(&self.snapshot.changed_files_set).contains(&path) {
                lock(&self.snapshot.semantic_diagnostics_per_file).insert(
                    path,
                    Arc::new(DiagnosticsOrBuildInfoDiagnosticsWithFileName::default()),
                );
            }
        }
        for diagnostic in &self.build_info.semantic_diagnostics_per_file {
            if diagnostic.file_id != 0 {
                let file_path = self.to_file_path(diagnostic.file_id);
                lock(&self.snapshot.semantic_diagnostics_per_file).remove(&file_path);
            // does not have cached diagnostics
            } else {
                let diagnostics = diagnostic
                    .diagnostics
                    .as_ref()
                    .expect("a semantic diagnostic without a file id has diagnostics");
                let file_path = self.to_file_path(diagnostics.file_id);
                let value =
                    self.to_diagnostics_or_build_info_diagnostics_with_file_name(diagnostics);
                lock(&self.snapshot.semantic_diagnostics_per_file)
                    .insert(file_path, Arc::new(value));
            }
        }
    }

    // port: tsc/internal/execute/incremental/buildinfotosnapshot.go:toSnapshot.setEmitDiagnostics
    fn set_emit_diagnostics(&mut self) {
        for diagnostic in &self.build_info.emit_diagnostics_per_file {
            let diagnostic = diagnostic
                .as_ref()
                .expect("an emit diagnostics entry of the build info");
            let file_path = self.to_file_path(diagnostic.file_id);
            let value = self.to_diagnostics_or_build_info_diagnostics_with_file_name(diagnostic);
            lock(&self.snapshot.emit_diagnostics_per_file).insert(file_path, Arc::new(value));
        }
    }

    // port: tsc/internal/execute/incremental/buildinfotosnapshot.go:toSnapshot.setAffectedFilesPendingEmit
    fn set_affected_files_pending_emit(&mut self) {
        if self.build_info.affected_files_pending_emit.is_empty() {
            return;
        }
        let own_options_emit_kind = get_file_emit_kind(&self.snapshot.options);
        for pending_emit in &self.build_info.affected_files_pending_emit {
            lock(&self.snapshot.affected_files_pending_emit).insert(
                self.to_file_path(pending_emit.file_id),
                if pending_emit.emit_kind == FileEmitKind::NONE {
                    own_options_emit_kind
                } else {
                    pending_emit.emit_kind
                },
            );
        }
    }

    // port: tsc/internal/execute/incremental/buildinfotosnapshot.go:toSnapshot.setPackageJsons
    fn set_package_jsons(&mut self) {
        let package_jsons = self
            .build_info
            .package_jsons
            .iter()
            .map(|path| self.to_absolute_path(path))
            .collect();
        let missing_package_jsons = self
            .build_info
            .missing_package_jsons
            .iter()
            .map(|path| self.to_absolute_path(path))
            .collect();
        let mut state = self.snapshot.state();
        state.package_jsons = Some(package_jsons);
        state.missing_package_jsons = Some(missing_package_jsons);
    }
}

// port: tsc/internal/execute/incremental/buildinfotosnapshot.go:fromBuildInfoRepopulateInfo
fn from_build_info_repopulate_info(
    info: Option<&BuildInfoRepopulateInfo>,
) -> Option<Arc<RepopulateDiagnosticInfo>> {
    let info = info?;
    Some(Arc::new(RepopulateDiagnosticInfo {
        kind: info.kind,
        module_reference: info.module_reference.clone(),
        mode: info.mode.0,
        package_name: info.package_name.clone(),
    }))
}
