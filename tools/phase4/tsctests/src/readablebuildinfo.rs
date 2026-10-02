//! `readablebuildinfo.go`: the `.tsbuildinfo.readable.baseline.txt` the fake
//! file system writes beside every build info, which spells file ids as file
//! names and pending-emit kinds as flag names.
//!
//! Each readable type encodes as the pin's struct does under
//! `json.MarshalIndent(&readable, "", "  ")`: fields in declaration order, an
//! `omitzero` field only when it is not its zero value. A slice the pin keeps
//! nil when it is empty is omitted when empty; the three the pin can leave
//! empty but non-nil (`root`, `fileInfos`, and a nil-checked pending-emit
//! list) follow the build info's own `Option`s.
use tsr_core::collections::OrderedMap;
use tsr_incremental::{
    get_file_emit_kind, AnyValue, BuildInfo, BuildInfoDiagnostic, BuildInfoDiagnosticsOfFile,
    BuildInfoEmitSignature, BuildInfoFileId, BuildInfoFileIdListId, BuildInfoFileInfo,
    BuildInfoFilePendingEmit, BuildInfoRepopulateInfo, BuildInfoRoot, BuildInfoSemanticDiagnostic,
    FileEmitKind,
};
use tsr_json::{Encode, Encoder, Error as JsonError, Token};
use tsr_jsstring::JsString;

/// A struct being encoded: its fields in order, an `omitzero` field only when
/// it is not its zero value.
struct Object<'e, 'a> {
    out: &'e mut Encoder<'a>,
}

impl<'e, 'a> Object<'e, 'a> {
    fn begin(out: &'e mut Encoder<'a>) -> Result<Self, JsonError> {
        out.write_token(Token::BeginObject)?;
        Ok(Self { out })
    }
    fn field(&mut self, name: &[u8], value: &(impl Encode + ?Sized)) -> Result<(), JsonError> {
        self.out.string(name)?;
        self.out.value(value)
    }
    fn string_omitzero(&mut self, name: &[u8], value: &[u8]) -> Result<(), JsonError> {
        if value.is_empty() {
            return Ok(());
        }
        self.out.string(name)?;
        self.out.string(value)
    }
    fn bool_omitzero(&mut self, name: &[u8], value: bool) -> Result<(), JsonError> {
        if !value {
            return Ok(());
        }
        self.field(name, &value)
    }
    fn int_omitzero(&mut self, name: &[u8], value: i64) -> Result<(), JsonError> {
        if value == 0 {
            return Ok(());
        }
        self.field(name, &value)
    }
    /// A slice the pin leaves nil when it has no entries.
    fn slice_omitzero<T: Encode>(&mut self, name: &[u8], value: &[T]) -> Result<(), JsonError> {
        if value.is_empty() {
            return Ok(());
        }
        self.field(name, value)
    }
    /// A pointer or a slice that may be non-nil and empty.
    fn option_omitzero<T: Encode + ?Sized>(
        &mut self,
        name: &[u8],
        value: Option<&T>,
    ) -> Result<(), JsonError> {
        match value {
            Some(value) => self.field(name, value),
            None => Ok(()),
        }
    }
    fn end(self) -> Result<(), JsonError> {
        self.out.write_token(Token::EndObject)
    }
}

/// `readableBuildInfo`.
struct ReadableBuildInfo<'b> {
    build_info: &'b BuildInfo,
    version: JsString,

    // Common between incremental and tsc -b buildinfo for non incremental programs
    errors: bool,
    check_pending: bool,
    root: Option<Vec<ReadableBuildInfoRoot<'b>>>,
    package_jsons: &'b [JsString],
    missing_package_jsons: &'b [JsString],

    // IncrementalProgram info
    file_names: &'b [JsString],
    file_infos: Option<Vec<ReadableBuildInfoFileInfo<'b>>>,
    file_ids_list: Vec<Vec<JsString>>,
    options: Option<&'b OrderedMap<JsString, AnyValue>>,
    referenced_map: Option<OrderedMap<JsString, Vec<JsString>>>,
    semantic_diagnostics_per_file: Vec<ReadableBuildInfoSemanticDiagnostic>,
    emit_diagnostics_per_file: Vec<ReadableBuildInfoDiagnosticsOfFile>,
    /// List of changed files in the program, not the whole set of files
    change_file_set: Vec<JsString>,
    affected_files_pending_emit: Option<Vec<ReadableBuildInfoFilePendingEmit<'b>>>,
    /// Because this is only output file in the program, we dont need fileId to deduplicate name
    latest_changed_dts_file: &'b JsString,
    emit_signatures: Vec<ReadableBuildInfoEmitSignature<'b>>,
    resolved_root: Vec<ReadableBuildInfoResolvedRoot>,
    /// Size of the build info file
    size: i64,

    // NonIncrementalProgram info
    semantic_errors: bool,
}

impl Encode for ReadableBuildInfo<'_> {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), JsonError> {
        let mut object = Object::begin(out)?;
        object.string_omitzero(b"version", self.version.as_bytes())?;
        object.bool_omitzero(b"errors", self.errors)?;
        object.bool_omitzero(b"checkPending", self.check_pending)?;
        object.option_omitzero(b"root", self.root.as_deref())?;
        object.slice_omitzero(b"packageJsons", self.package_jsons)?;
        object.slice_omitzero(b"missingPackageJsons", self.missing_package_jsons)?;
        object.slice_omitzero(b"fileNames", self.file_names)?;
        object.option_omitzero(b"fileInfos", self.file_infos.as_deref())?;
        object.slice_omitzero(b"fileIdsList", &self.file_ids_list)?;
        object.option_omitzero(b"options", self.options)?;
        object.option_omitzero(b"referencedMap", self.referenced_map.as_ref())?;
        object.slice_omitzero(
            b"semanticDiagnosticsPerFile",
            &self.semantic_diagnostics_per_file,
        )?;
        object.slice_omitzero(b"emitDiagnosticsPerFile", &self.emit_diagnostics_per_file)?;
        object.slice_omitzero(b"changeFileSet", &self.change_file_set)?;
        object.option_omitzero(
            b"affectedFilesPendingEmit",
            self.affected_files_pending_emit.as_deref(),
        )?;
        object.string_omitzero(
            b"latestChangedDtsFile",
            self.latest_changed_dts_file.as_bytes(),
        )?;
        object.slice_omitzero(b"emitSignatures", &self.emit_signatures)?;
        object.slice_omitzero(b"resolvedRoot", &self.resolved_root)?;
        object.int_omitzero(b"size", self.size)?;
        object.bool_omitzero(b"semanticErrors", self.semantic_errors)?;
        object.end()
    }
}

/// `readableBuildInfoRoot`.
struct ReadableBuildInfoRoot<'b> {
    files: Vec<JsString>,
    original: &'b BuildInfoRoot,
}

impl Encode for ReadableBuildInfoRoot<'_> {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), JsonError> {
        let mut object = Object::begin(out)?;
        // `files` is never nil.
        object.field(b"files", &self.files)?;
        object.field(b"original", self.original)?;
        object.end()
    }
}

/// `readableBuildInfoFileInfo`.
struct ReadableBuildInfoFileInfo<'b> {
    file_name: JsString,
    version: JsString,
    signature: JsString,
    affects_global_scope: bool,
    implied_node_format: String,
    /// Original file path, if available
    original: Option<&'b BuildInfoFileInfo>,
}

impl Encode for ReadableBuildInfoFileInfo<'_> {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), JsonError> {
        let mut object = Object::begin(out)?;
        object.string_omitzero(b"fileName", self.file_name.as_bytes())?;
        object.string_omitzero(b"version", self.version.as_bytes())?;
        object.string_omitzero(b"signature", self.signature.as_bytes())?;
        object.bool_omitzero(b"affectsGlobalScope", self.affects_global_scope)?;
        object.string_omitzero(b"impliedNodeFormat", self.implied_node_format.as_bytes())?;
        object.option_omitzero(b"original", self.original)?;
        object.end()
    }
}

/// `readableBuildInfoDiagnostic`.
struct ReadableBuildInfoDiagnostic {
    /// incrementalBuildInfoFileId if it is for a File thats other than its stored for
    file: JsString,
    no_file: bool,
    pos: i64,
    end: i64,
    code: i32,
    category: i32,
    message_key: JsString,
    message_args: Vec<JsString>,
    message_chain: Vec<ReadableBuildInfoDiagnostic>,
    related_information: Vec<ReadableBuildInfoDiagnostic>,
    reports_unnecessary: bool,
    reports_deprecated: bool,
    skipped_on_no_emit: bool,
    repopulate_info: Option<ReadableBuildInfoRepopulateInfo>,
}

impl Encode for ReadableBuildInfoDiagnostic {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), JsonError> {
        let mut object = Object::begin(out)?;
        object.string_omitzero(b"file", self.file.as_bytes())?;
        object.bool_omitzero(b"noFile", self.no_file)?;
        object.int_omitzero(b"pos", self.pos)?;
        object.int_omitzero(b"end", self.end)?;
        object.int_omitzero(b"code", i64::from(self.code))?;
        object.int_omitzero(b"category", i64::from(self.category))?;
        object.string_omitzero(b"messageKey", self.message_key.as_bytes())?;
        object.slice_omitzero(b"messageArgs", &self.message_args)?;
        object.slice_omitzero(b"messageChain", &self.message_chain)?;
        object.slice_omitzero(b"relatedInformation", &self.related_information)?;
        object.bool_omitzero(b"reportsUnnecessary", self.reports_unnecessary)?;
        object.bool_omitzero(b"reportsDeprecated", self.reports_deprecated)?;
        object.bool_omitzero(b"skippedOnNoEmit", self.skipped_on_no_emit)?;
        object.option_omitzero(b"repopulateInfo", self.repopulate_info.as_ref())?;
        object.end()
    }
}

/// `readableBuildInfoRepopulateInfo`.
struct ReadableBuildInfoRepopulateInfo {
    kind: i32,
    module_reference: JsString,
    mode: i32,
    package_name: JsString,
}

impl Encode for ReadableBuildInfoRepopulateInfo {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), JsonError> {
        let mut object = Object::begin(out)?;
        object.field(b"kind", &i64::from(self.kind))?;
        object.string_omitzero(b"moduleReference", self.module_reference.as_bytes())?;
        object.int_omitzero(b"mode", i64::from(self.mode))?;
        object.string_omitzero(b"packageName", self.package_name.as_bytes())?;
        object.end()
    }
}

/// `readableBuildInfoDiagnosticsOfFile`: `[file, diagnostics]`.
struct ReadableBuildInfoDiagnosticsOfFile {
    file: JsString,
    diagnostics: Vec<ReadableBuildInfoDiagnostic>,
}

impl Encode for ReadableBuildInfoDiagnosticsOfFile {
    // port: tsc/internal/execute/tsctests/readablebuildinfo.go:readableBuildInfoDiagnosticsOfFile.MarshalJSON
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), JsonError> {
        out.write_token(Token::BeginArray)?;
        out.value(&self.file)?;
        // A nil slice inside `[]any` marshals as `[]`.
        out.value(&self.diagnostics)?;
        out.write_token(Token::EndArray)
    }
}

/// `readableBuildInfoSemanticDiagnostic`.
struct ReadableBuildInfoSemanticDiagnostic {
    /// File is not in changedSet and still doesnt have cached diagnostics
    file: JsString,
    /// Diagnostics for file
    diagnostics: Option<ReadableBuildInfoDiagnosticsOfFile>,
}

impl Encode for ReadableBuildInfoSemanticDiagnostic {
    // port: tsc/internal/execute/tsctests/readablebuildinfo.go:readableBuildInfoSemanticDiagnostic.MarshalJSON
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), JsonError> {
        if !self.file.is_empty() {
            return out.value(&self.file);
        }
        out.value(&self.diagnostics)
    }
}

/// `readableBuildInfoFilePendingEmit`.
struct ReadableBuildInfoFilePendingEmit<'b> {
    file: JsString,
    emit_kind: String,
    original: &'b BuildInfoFilePendingEmit,
}

impl Encode for ReadableBuildInfoFilePendingEmit<'_> {
    // port: tsc/internal/execute/tsctests/readablebuildinfo.go:readableBuildInfoFilePendingEmit.MarshalJSON
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), JsonError> {
        out.write_token(Token::BeginArray)?;
        out.value(&self.file)?;
        out.value(self.emit_kind.as_str())?;
        out.value(self.original)?;
        out.write_token(Token::EndArray)
    }
}

/// `readableBuildInfoEmitSignature`.
struct ReadableBuildInfoEmitSignature<'b> {
    file: JsString,
    signature: &'b JsString,
    differs_only_in_dts_map: bool,
    differs_in_options: bool,
    original: &'b BuildInfoEmitSignature,
}

impl Encode for ReadableBuildInfoEmitSignature<'_> {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), JsonError> {
        let mut object = Object::begin(out)?;
        object.string_omitzero(b"file", self.file.as_bytes())?;
        object.string_omitzero(b"signature", self.signature.as_bytes())?;
        object.bool_omitzero(b"differsOnlyInDtsMap", self.differs_only_in_dts_map)?;
        object.bool_omitzero(b"differsInOptions", self.differs_in_options)?;
        object.field(b"original", self.original)?;
        object.end()
    }
}

/// `readableBuildInfoResolvedRoot`.
struct ReadableBuildInfoResolvedRoot {
    resolved: JsString,
    root: JsString,
}

impl Encode for ReadableBuildInfoResolvedRoot {
    // port: tsc/internal/execute/tsctests/readablebuildinfo.go:readableBuildInfoResolvedRoot.MarshalJSON
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), JsonError> {
        out.value(&[&self.resolved, &self.root][..])
    }
}

/// The readable rendering of `build_info`, whose text is `build_info_text`
/// (its size is the rendering's `size`).
///
/// # Panics
/// On a file id outside `fileNames` or a file-id list id outside
/// `fileIdsList`, as the pin's slice indexing does, and when the rendering
/// cannot be marshaled.
// port: tsc/internal/execute/tsctests/readablebuildinfo.go:toReadableBuildInfo
pub fn to_readable_build_info(build_info: &BuildInfo, build_info_text: &[u8]) -> Vec<u8> {
    let mut readable = ReadableBuildInfo {
        build_info,
        version: build_info.version.clone(),
        errors: build_info.errors,
        check_pending: build_info.check_pending,
        root: None,
        package_jsons: &build_info.package_jsons,
        missing_package_jsons: &build_info.missing_package_jsons,
        file_names: &build_info.file_names,
        file_infos: None,
        file_ids_list: Vec::new(),
        options: build_info.options.as_ref(),
        referenced_map: None,
        semantic_diagnostics_per_file: Vec::new(),
        emit_diagnostics_per_file: Vec::new(),
        change_file_set: Vec::new(),
        affected_files_pending_emit: None,
        latest_changed_dts_file: &build_info.latest_changed_dts_file,
        emit_signatures: Vec::new(),
        resolved_root: Vec::new(),
        size: build_info_text.len() as i64,
        semantic_errors: build_info.semantic_errors,
    };
    readable.set_file_infos();
    readable.set_root();
    readable.set_file_ids_list();
    readable.set_referenced_map();
    readable.set_change_file_set();
    readable.set_semantic_diagnostics();
    readable.set_emit_diagnostics();
    readable.set_affected_files_pending_emit();
    readable.set_emit_signatures();
    readable.set_resolved_root();
    match tsr_json::marshal_indent(&readable, "", "  ") {
        Ok(contents) => contents,
        Err(error) => {
            panic!("readableBuildInfo: failed to marshal readable build info: {error}")
        }
    }
}

impl ReadableBuildInfo<'_> {
    // port: tsc/internal/execute/tsctests/readablebuildinfo.go:readableBuildInfo.toFilePath
    fn to_file_path(&self, file_id: BuildInfoFileId) -> JsString {
        self.build_info.file_names[(file_id - 1) as usize].clone()
    }

    // port: tsc/internal/execute/tsctests/readablebuildinfo.go:readableBuildInfo.toFilePathSet
    fn to_file_path_set(&self, file_id_list_id: BuildInfoFileIdListId) -> Vec<JsString> {
        self.file_ids_list[(file_id_list_id - 1) as usize].clone()
    }

    // port: tsc/internal/execute/tsctests/readablebuildinfo.go:readableBuildInfo.toReadableBuildInfoDiagnostic
    fn to_readable_build_info_diagnostic(
        &self,
        diagnostics: &[BuildInfoDiagnostic],
    ) -> Vec<ReadableBuildInfoDiagnostic> {
        diagnostics
            .iter()
            .map(|d| {
                let file = if d.file != 0 {
                    self.to_file_path(d.file)
                } else {
                    JsString::default()
                };
                ReadableBuildInfoDiagnostic {
                    file,
                    no_file: d.no_file,
                    pos: d.pos,
                    end: d.end,
                    code: d.code,
                    category: d.category,
                    message_key: d.message_key.clone(),
                    message_args: d.message_args.clone(),
                    message_chain: self.to_readable_build_info_diagnostic(&d.message_chain),
                    related_information: self
                        .to_readable_build_info_diagnostic(&d.related_information),
                    reports_unnecessary: d.reports_unnecessary,
                    reports_deprecated: d.reports_deprecated,
                    skipped_on_no_emit: d.skipped_on_no_emit,
                    repopulate_info: to_readable_build_info_repopulate_info(
                        d.repopulate_info.as_ref(),
                    ),
                }
            })
            .collect()
    }

    // port: tsc/internal/execute/tsctests/readablebuildinfo.go:readableBuildInfo.toReadableBuildInfoDiagnosticsOfFile
    fn to_readable_build_info_diagnostics_of_file(
        &self,
        diagnostics: &BuildInfoDiagnosticsOfFile,
    ) -> ReadableBuildInfoDiagnosticsOfFile {
        ReadableBuildInfoDiagnosticsOfFile {
            file: self.to_file_path(diagnostics.file_id),
            diagnostics: self.to_readable_build_info_diagnostic(&diagnostics.diagnostics),
        }
    }

    // port: tsc/internal/execute/tsctests/readablebuildinfo.go:readableBuildInfo.setFileInfos
    fn set_file_infos(&mut self) {
        self.file_infos = self.build_info.file_infos.as_ref().map(|file_infos| {
            file_infos
                .iter()
                .enumerate()
                .map(|(index, original)| {
                    let file_info = BuildInfoFileInfo::get_file_info(Some(original))
                        .expect("a present file info has a file info");
                    // Dont set original for string encoding
                    let original = (!original.has_signature()).then_some(original);
                    ReadableBuildInfoFileInfo {
                        file_name: self.to_file_path(index as BuildInfoFileId + 1),
                        version: file_info.version().clone(),
                        signature: file_info.signature().clone(),
                        affects_global_scope: file_info.affects_global_scope(),
                        implied_node_format: file_info.implied_node_format().to_string(),
                        original,
                    }
                })
                .collect()
        });
    }

    // port: tsc/internal/execute/tsctests/readablebuildinfo.go:readableBuildInfo.setRoot
    fn set_root(&mut self) {
        self.root = self.build_info.root.as_ref().map(|roots| {
            roots
                .iter()
                .map(|original| {
                    let files = if !original.non_incremental.is_empty() {
                        vec![original.non_incremental.clone()]
                    } else if original.end == 0 {
                        vec![self.to_file_path(original.start)]
                    } else {
                        (original.start..=original.end)
                            .map(|i| self.to_file_path(i))
                            .collect()
                    };
                    ReadableBuildInfoRoot { files, original }
                })
                .collect()
        });
    }

    // port: tsc/internal/execute/tsctests/readablebuildinfo.go:readableBuildInfo.setFileIdsList
    fn set_file_ids_list(&mut self) {
        self.file_ids_list = self
            .build_info
            .file_ids_list
            .iter()
            .map(|ids| ids.iter().map(|&id| self.to_file_path(id)).collect())
            .collect();
    }

    // port: tsc/internal/execute/tsctests/readablebuildinfo.go:readableBuildInfo.setReferencedMap
    fn set_referenced_map(&mut self) {
        if !self.build_info.referenced_map.is_empty() {
            let mut referenced_map = OrderedMap::default();
            for entry in &self.build_info.referenced_map {
                referenced_map.insert(
                    self.to_file_path(entry.file_id),
                    self.to_file_path_set(entry.file_id_list_id),
                );
            }
            self.referenced_map = Some(referenced_map);
        }
    }

    // port: tsc/internal/execute/tsctests/readablebuildinfo.go:readableBuildInfo.setChangeFileSet
    fn set_change_file_set(&mut self) {
        self.change_file_set = self
            .build_info
            .change_file_set
            .iter()
            .map(|&id| self.to_file_path(id))
            .collect();
    }

    // port: tsc/internal/execute/tsctests/readablebuildinfo.go:readableBuildInfo.setSemanticDiagnostics
    fn set_semantic_diagnostics(&mut self) {
        self.semantic_diagnostics_per_file = self
            .build_info
            .semantic_diagnostics_per_file
            .iter()
            .map(|diagnostics: &BuildInfoSemanticDiagnostic| {
                if diagnostics.file_id != 0 {
                    return ReadableBuildInfoSemanticDiagnostic {
                        file: self.to_file_path(diagnostics.file_id),
                        diagnostics: None,
                    };
                }
                let of_file = diagnostics
                    .diagnostics
                    .as_ref()
                    .expect("a semantic-diagnostics entry without a file id has diagnostics");
                ReadableBuildInfoSemanticDiagnostic {
                    file: JsString::default(),
                    diagnostics: Some(self.to_readable_build_info_diagnostics_of_file(of_file)),
                }
            })
            .collect();
    }

    // port: tsc/internal/execute/tsctests/readablebuildinfo.go:readableBuildInfo.setEmitDiagnostics
    fn set_emit_diagnostics(&mut self) {
        self.emit_diagnostics_per_file = self
            .build_info
            .emit_diagnostics_per_file
            .iter()
            .map(|diagnostics| {
                // The pin dereferences a nil entry.
                let diagnostics = diagnostics
                    .as_ref()
                    .expect("runtime error: invalid memory address or nil pointer dereference");
                self.to_readable_build_info_diagnostics_of_file(diagnostics)
            })
            .collect();
    }

    // port: tsc/internal/execute/tsctests/readablebuildinfo.go:readableBuildInfo.setAffectedFilesPendingEmit
    fn set_affected_files_pending_emit(&mut self) {
        if self.build_info.affected_files_pending_emit.is_empty() {
            return;
        }
        let full_emit_kind = get_file_emit_kind(&self.build_info.get_compiler_options(b""));
        self.affected_files_pending_emit = Some(
            self.build_info
                .affected_files_pending_emit
                .iter()
                .map(|pending_emit| {
                    let emit_kind = if pending_emit.emit_kind == FileEmitKind::NONE {
                        full_emit_kind
                    } else {
                        pending_emit.emit_kind
                    };
                    ReadableBuildInfoFilePendingEmit {
                        file: self.to_file_path(pending_emit.file_id),
                        emit_kind: to_readable_file_emit_kind(emit_kind),
                        original: pending_emit,
                    }
                })
                .collect(),
        );
    }

    // port: tsc/internal/execute/tsctests/readablebuildinfo.go:readableBuildInfo.setEmitSignatures
    fn set_emit_signatures(&mut self) {
        let build_info = self.build_info;
        self.emit_signatures = build_info
            .emit_signatures
            .iter()
            .map(|signature| ReadableBuildInfoEmitSignature {
                file: self.to_file_path(signature.file_id),
                signature: &signature.signature,
                differs_only_in_dts_map: signature.differs_only_in_dts_map,
                differs_in_options: signature.differs_in_options,
                original: signature,
            })
            .collect();
    }

    // port: tsc/internal/execute/tsctests/readablebuildinfo.go:readableBuildInfo.setResolvedRoot
    fn set_resolved_root(&mut self) {
        self.resolved_root = self
            .build_info
            .resolved_root
            .iter()
            .map(|original| ReadableBuildInfoResolvedRoot {
                resolved: self.to_file_path(original.resolved),
                root: self.to_file_path(original.root),
            })
            .collect();
    }
}

// port: tsc/internal/execute/tsctests/readablebuildinfo.go:toReadableBuildInfoRepopulateInfo
fn to_readable_build_info_repopulate_info(
    info: Option<&BuildInfoRepopulateInfo>,
) -> Option<ReadableBuildInfoRepopulateInfo> {
    let info = info?;
    Some(ReadableBuildInfoRepopulateInfo {
        kind: info.kind,
        module_reference: info.module_reference.clone(),
        mode: info.mode.0,
        package_name: info.package_name.clone(),
    })
}

/// The pending-emit flags by name, `|`-separated, or `None`.
// port: tsc/internal/execute/tsctests/readablebuildinfo.go:toReadableFileEmitKind
pub fn to_readable_file_emit_kind(file_emit_kind: FileEmitKind) -> String {
    let mut builder = String::new();
    let mut add_flags = |flags: &str| {
        if builder.is_empty() {
            builder.push_str(flags);
        } else {
            builder.push('|');
            builder.push_str(flags);
        }
    };
    if file_emit_kind != FileEmitKind::NONE {
        if (file_emit_kind & FileEmitKind::JS) != FileEmitKind::NONE {
            add_flags("Js");
        }
        if (file_emit_kind & FileEmitKind::JS_MAP) != FileEmitKind::NONE {
            add_flags("JsMap");
        }
        if (file_emit_kind & FileEmitKind::JS_INLINE_MAP) != FileEmitKind::NONE {
            add_flags("JsInlineMap");
        }
        if (file_emit_kind & FileEmitKind::DTS) == FileEmitKind::DTS {
            add_flags("Dts");
        } else {
            if (file_emit_kind & FileEmitKind::DTS_EMIT) != FileEmitKind::NONE {
                add_flags("DtsEmit");
            }
            if (file_emit_kind & FileEmitKind::DTS_ERRORS) != FileEmitKind::NONE {
                add_flags("DtsErrors");
            }
        }
        if (file_emit_kind & FileEmitKind::DTS_MAP) != FileEmitKind::NONE {
            add_flags("DtsMap");
        }
    }
    if !builder.is_empty() {
        return builder;
    }
    "None".to_owned()
}
