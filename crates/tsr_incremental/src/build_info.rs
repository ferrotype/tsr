//! The build info (`buildInfo.go`): what an incremental program writes to its
//! `.tsbuildinfo` and reads back, with the pin's JSON shapes. Each type's
//! [`Encode`] is its `MarshalJSON` and its [`Decode`] its `UnmarshalJSON`;
//! a struct's fields encode in declaration order, a field tagged `omitzero`
//! only when it is not its zero value, as the pin's `json.Marshal`
//! (go-json-experiment) writes them.
use crate::json::{self, AnyValue};
use crate::snapshot::{
    get_pending_emit_kind_with_options, EmitSignature, FileEmitKind, FileInfo, Path,
};
use std::collections::HashMap;
use std::sync::Mutex;
use tsr_core::collections::OrderedMap;
use tsr_core::{CompilerOptions, ModuleKind};
use tsr_json::{Decode, Decoder, Encode, Encoder, Error as JsonError, Kind};
use tsr_jsstring::JsString;
use tsr_tsoptions::ParsedCommandLine;

/// `BuildInfoFileId`: a 1-based index into `fileNames`.
pub type BuildInfoFileId = i64;
/// `BuildInfoFileIdListId`: a 1-based index into `fileIdsList`.
pub type BuildInfoFileIdListId = i64;

/// buildInfoRoot is
/// - for incremental program buildinfo
///   - start and end of FileId for consecutive fileIds to be included as root
///   - start - single fileId that is root
///
/// - for non incremental program buildinfo
///   - string that is the root file name
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildInfoRoot {
    pub start: BuildInfoFileId,
    pub end: BuildInfoFileId,
    /// Root of a non incremental program
    pub non_incremental: JsString,
}

impl Encode for BuildInfoRoot {
    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfoRoot.MarshalJSON
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), JsonError> {
        if self.start != 0 {
            if self.end != 0 {
                out.value(&[self.start, self.end][..])
            } else {
                out.value(&self.start)
            }
        } else {
            out.value(&self.non_incremental)
        }
    }
}

impl Decode for BuildInfoRoot {
    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfoRoot.UnmarshalJSON
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), JsonError> {
        let data = input.read_value()?;
        match json::unmarshal_pair(&data) {
            Err(_) => match json::unmarshal::<i64>(&data) {
                Err(_) => match json::unmarshal::<JsString>(&data) {
                    Err(_) => Err(json::invalid("BuildInfoRoot", &data)),
                    Ok(name) => {
                        *self = Self {
                            non_incremental: name,
                            ..Self::default()
                        };
                        Ok(())
                    }
                },
                Ok(start) => {
                    *self = Self {
                        start,
                        ..Self::default()
                    };
                    Ok(())
                }
            },
            Ok([start, end]) => {
                *self = Self {
                    start,
                    end,
                    ..Self::default()
                };
                Ok(())
            }
        }
    }
}

/// `buildInfoFileInfoNoSignature`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildInfoFileInfoNoSignature {
    pub version: JsString,
    pub no_signature: bool,
    pub affects_global_scope: bool,
    pub implied_node_format: ModuleKind,
}

impl Encode for BuildInfoFileInfoNoSignature {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), JsonError> {
        let mut object = json::Object::begin(out)?;
        object.string_omitzero(b"version", &self.version)?;
        object.bool_omitzero(b"noSignature", self.no_signature)?;
        object.bool_omitzero(b"affectsGlobalScope", self.affects_global_scope)?;
        object.int_omitzero(b"impliedNodeFormat", i64::from(self.implied_node_format.0))?;
        object.end()
    }
}

impl Decode for BuildInfoFileInfoNoSignature {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), JsonError> {
        json::object(input, |name, input| match name {
            b"version" => input.value(&mut self.version),
            b"noSignature" => input.value(&mut self.no_signature),
            b"affectsGlobalScope" => input.value(&mut self.affects_global_scope),
            b"impliedNodeFormat" => input.value(&mut self.implied_node_format.0),
            _ => input.skip_value(),
        })
    }
}

/// `buildInfoFileInfoWithSignature`. Signature is
/// - undefined if FileInfo.version === FileInfo.signature
/// - string actual signature
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildInfoFileInfoWithSignature {
    pub version: JsString,
    pub signature: JsString,
    pub affects_global_scope: bool,
    pub implied_node_format: ModuleKind,
}

impl Encode for BuildInfoFileInfoWithSignature {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), JsonError> {
        let mut object = json::Object::begin(out)?;
        object.string_omitzero(b"version", &self.version)?;
        object.string_omitzero(b"signature", &self.signature)?;
        object.bool_omitzero(b"affectsGlobalScope", self.affects_global_scope)?;
        object.int_omitzero(b"impliedNodeFormat", i64::from(self.implied_node_format.0))?;
        object.end()
    }
}

impl Decode for BuildInfoFileInfoWithSignature {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), JsonError> {
        json::object(input, |name, input| match name {
            b"version" => input.value(&mut self.version),
            b"signature" => input.value(&mut self.signature),
            b"affectsGlobalScope" => input.value(&mut self.affects_global_scope),
            b"impliedNodeFormat" => input.value(&mut self.implied_node_format.0),
            _ => input.skip_value(),
        })
    }
}

/// `BuildInfoFileInfo`: a file's version and signature in one of three
/// shapes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildInfoFileInfo {
    signature: JsString,
    no_signature: Option<BuildInfoFileInfoNoSignature>,
    file_info: Option<BuildInfoFileInfoWithSignature>,
}

// port: tsc/internal/execute/incremental/buildInfo.go:newBuildInfoFileInfo
pub(crate) fn new_build_info_file_info(file_info: &FileInfo) -> BuildInfoFileInfo {
    if file_info.version == file_info.signature {
        if !file_info.affects_global_scope && file_info.implied_node_format == ModuleKind::COMMON_JS
        {
            return BuildInfoFileInfo {
                signature: file_info.signature.clone(),
                ..BuildInfoFileInfo::default()
            };
        }
    } else if file_info.signature.is_empty() {
        return BuildInfoFileInfo {
            no_signature: Some(BuildInfoFileInfoNoSignature {
                version: file_info.version.clone(),
                no_signature: true,
                affects_global_scope: file_info.affects_global_scope,
                implied_node_format: file_info.implied_node_format,
            }),
            ..BuildInfoFileInfo::default()
        };
    }
    BuildInfoFileInfo {
        file_info: Some(BuildInfoFileInfoWithSignature {
            version: file_info.version.clone(),
            signature: if file_info.signature == file_info.version {
                JsString::default()
            } else {
                file_info.signature.clone()
            },
            affects_global_scope: file_info.affects_global_scope,
            implied_node_format: file_info.implied_node_format,
        }),
        ..BuildInfoFileInfo::default()
    }
}

impl BuildInfoFileInfo {
    /// `None` is the pin's nil receiver.
    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfoFileInfo.GetFileInfo
    pub fn get_file_info(this: Option<&Self>) -> Option<FileInfo> {
        let b = this?;
        if !b.signature.is_empty() {
            return Some(FileInfo {
                version: b.signature.clone(),
                signature: b.signature.clone(),
                affects_global_scope: false,
                implied_node_format: ModuleKind::COMMON_JS,
            });
        }
        if let Some(no_signature) = &b.no_signature {
            return Some(FileInfo {
                version: no_signature.version.clone(),
                signature: JsString::default(),
                affects_global_scope: no_signature.affects_global_scope,
                implied_node_format: no_signature.implied_node_format,
            });
        }
        let file_info = b
            .file_info
            .as_ref()
            .expect("a build-info file info has one of its three shapes");
        Some(FileInfo {
            version: file_info.version.clone(),
            signature: if file_info.signature.is_empty() {
                file_info.version.clone()
            } else {
                file_info.signature.clone()
            },
            affects_global_scope: file_info.affects_global_scope,
            implied_node_format: file_info.implied_node_format,
        })
    }

    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfoFileInfo.HasSignature
    pub fn has_signature(&self) -> bool {
        !self.signature.is_empty()
    }
}

impl Encode for BuildInfoFileInfo {
    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfoFileInfo.MarshalJSON
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), JsonError> {
        if !self.signature.is_empty() {
            return out.value(&self.signature);
        }
        if let Some(no_signature) = &self.no_signature {
            return out.value(no_signature);
        }
        match &self.file_info {
            Some(file_info) => out.value(file_info),
            None => out.null(),
        }
    }
}

impl Decode for BuildInfoFileInfo {
    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfoFileInfo.UnmarshalJSON
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), JsonError> {
        let data = input.read_value()?;
        match json::unmarshal::<JsString>(&data) {
            Err(_) => match json::unmarshal::<BuildInfoFileInfoNoSignature>(&data) {
                Ok(no_signature) if no_signature.no_signature => {
                    *self = Self {
                        no_signature: Some(no_signature),
                        ..Self::default()
                    };
                    Ok(())
                }
                _ => match json::unmarshal::<BuildInfoFileInfoWithSignature>(&data) {
                    Err(_) => Err(json::invalid("BuildInfoFileInfo", &data)),
                    Ok(file_info) => {
                        *self = Self {
                            file_info: Some(file_info),
                            ..Self::default()
                        };
                        Ok(())
                    }
                },
            },
            Ok(signature) => {
                *self = Self {
                    signature,
                    ..Self::default()
                };
                Ok(())
            }
        }
    }
}

/// `BuildInfoReferenceMapEntry`: `[fileId, fileIdListId]`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildInfoReferenceMapEntry {
    pub file_id: BuildInfoFileId,
    pub file_id_list_id: BuildInfoFileIdListId,
}

impl Encode for BuildInfoReferenceMapEntry {
    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfoReferenceMapEntry.MarshalJSON
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), JsonError> {
        out.value(&[self.file_id, self.file_id_list_id][..])
    }
}

impl Decode for BuildInfoReferenceMapEntry {
    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfoReferenceMapEntry.UnmarshalJSON
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), JsonError> {
        let data = input.read_value()?;
        let [file_id, file_id_list_id] = json::unmarshal_pair(&data)?;
        *self = Self {
            file_id,
            file_id_list_id,
        };
        Ok(())
    }
}

/// `BuildInfoDiagnostic`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildInfoDiagnostic {
    /// BuildInfoFileId if it is for a File thats other than its stored for
    pub file: BuildInfoFileId,
    pub no_file: bool,
    pub pos: i64,
    pub end: i64,
    pub code: i32,
    pub category: i32,
    pub source: JsString,
    pub message_text: JsString,
    pub message_key: JsString,
    pub message_args: Vec<JsString>,
    pub message_chain: Vec<BuildInfoDiagnostic>,
    pub related_information: Vec<BuildInfoDiagnostic>,
    pub reports_unnecessary: bool,
    pub reports_deprecated: bool,
    pub skipped_on_no_emit: bool,
    pub repopulate_info: Option<BuildInfoRepopulateInfo>,
}

impl Encode for BuildInfoDiagnostic {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), JsonError> {
        let mut object = json::Object::begin(out)?;
        object.int_omitzero(b"file", self.file)?;
        object.bool_omitzero(b"noFile", self.no_file)?;
        object.int_omitzero(b"pos", self.pos)?;
        object.int_omitzero(b"end", self.end)?;
        object.int_omitzero(b"code", i64::from(self.code))?;
        object.int_omitzero(b"category", i64::from(self.category))?;
        object.string_omitzero(b"source", &self.source)?;
        object.string_omitzero(b"messageText", &self.message_text)?;
        object.string_omitzero(b"messageKey", &self.message_key)?;
        object.slice_omitzero(b"messageArgs", &self.message_args)?;
        object.slice_omitzero(b"messageChain", &self.message_chain)?;
        object.slice_omitzero(b"relatedInformation", &self.related_information)?;
        object.bool_omitzero(b"reportsUnnecessary", self.reports_unnecessary)?;
        object.bool_omitzero(b"reportsDeprecated", self.reports_deprecated)?;
        object.bool_omitzero(b"skippedOnNoEmit", self.skipped_on_no_emit)?;
        if let Some(info) = &self.repopulate_info {
            object.field(b"repopulateInfo", info)?;
        }
        object.end()
    }
}

impl Decode for BuildInfoDiagnostic {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), JsonError> {
        json::object(input, |name, input| match name {
            b"file" => input.value(&mut self.file),
            b"noFile" => input.value(&mut self.no_file),
            b"pos" => input.value(&mut self.pos),
            b"end" => input.value(&mut self.end),
            b"code" => input.value(&mut self.code),
            b"category" => input.value(&mut self.category),
            b"source" => input.value(&mut self.source),
            b"messageText" => input.value(&mut self.message_text),
            b"messageKey" => input.value(&mut self.message_key),
            b"messageArgs" => input.value(&mut self.message_args),
            b"messageChain" => input.value(&mut self.message_chain),
            b"relatedInformation" => input.value(&mut self.related_information),
            b"reportsUnnecessary" => input.value(&mut self.reports_unnecessary),
            b"reportsDeprecated" => input.value(&mut self.reports_deprecated),
            b"skippedOnNoEmit" => input.value(&mut self.skipped_on_no_emit),
            b"repopulateInfo" => input.value(&mut self.repopulate_info),
            _ => input.skip_value(),
        })
    }
}

/// `BuildInfoRepopulateInfo`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildInfoRepopulateInfo {
    pub kind: i32,
    pub module_reference: JsString,
    pub mode: ModuleKind,
    pub package_name: JsString,
}

impl Encode for BuildInfoRepopulateInfo {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), JsonError> {
        let mut object = json::Object::begin(out)?;
        object.field(b"kind", &i64::from(self.kind))?;
        object.string_omitzero(b"moduleReference", &self.module_reference)?;
        object.int_omitzero(b"mode", i64::from(self.mode.0))?;
        object.string_omitzero(b"packageName", &self.package_name)?;
        object.end()
    }
}

impl Decode for BuildInfoRepopulateInfo {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), JsonError> {
        json::object(input, |name, input| match name {
            b"kind" => input.value(&mut self.kind),
            b"moduleReference" => input.value(&mut self.module_reference),
            b"mode" => input.value(&mut self.mode.0),
            b"packageName" => input.value(&mut self.package_name),
            _ => input.skip_value(),
        })
    }
}

/// `BuildInfoDiagnosticsOfFile`: `[fileId, diagnostics]`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildInfoDiagnosticsOfFile {
    pub file_id: BuildInfoFileId,
    pub diagnostics: Vec<BuildInfoDiagnostic>,
}

impl Encode for BuildInfoDiagnosticsOfFile {
    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfoDiagnosticsOfFile.MarshalJSON
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), JsonError> {
        out.write_token(tsr_json::Token::BeginArray)?;
        out.value(&self.file_id)?;
        out.value(&self.diagnostics)?;
        out.write_token(tsr_json::Token::EndArray)
    }
}

impl Decode for BuildInfoDiagnosticsOfFile {
    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfoDiagnosticsOfFile.UnmarshalJSON
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), JsonError> {
        let data = input.read_value()?;
        let Ok(file_id_and_diagnostics) = json::unmarshal::<Vec<tsr_json::RawValue>>(&data) else {
            return Err(json::invalid("BuildInfoDiagnosticsOfFile", &data));
        };
        if file_id_and_diagnostics.len() != 2 {
            return Err(JsonError::Message(format!(
                "invalid BuildInfoDiagnosticsOfFile: expected 2 elements, got {}",
                file_id_and_diagnostics.len()
            )));
        }
        let file_id =
            json::unmarshal::<BuildInfoFileId>(&file_id_and_diagnostics[0].0).map_err(|error| {
                JsonError::Message(format!(
                    "invalid fileId in BuildInfoDiagnosticsOfFile: {error}"
                ))
            })?;
        let diagnostics = json::unmarshal::<Vec<BuildInfoDiagnostic>>(
            &file_id_and_diagnostics[1].0,
        )
        .map_err(|error| {
            JsonError::Message(format!(
                "invalid diagnostics in BuildInfoDiagnosticsOfFile: {error}"
            ))
        })?;
        *self = Self {
            file_id,
            diagnostics,
        };
        Ok(())
    }
}

/// `BuildInfoSemanticDiagnostic`: a file id when the file is not in the
/// changed set and still has no cached diagnostics, else its diagnostics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildInfoSemanticDiagnostic {
    /// File is not in changedSet and still doesnt have cached diagnostics
    pub file_id: BuildInfoFileId,
    /// Diagnostics for file
    pub diagnostics: Option<BuildInfoDiagnosticsOfFile>,
}

impl Encode for BuildInfoSemanticDiagnostic {
    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfoSemanticDiagnostic.MarshalJSON
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), JsonError> {
        if self.file_id != 0 {
            return out.value(&self.file_id);
        }
        out.value(&self.diagnostics)
    }
}

impl Decode for BuildInfoSemanticDiagnostic {
    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfoSemanticDiagnostic.UnmarshalJSON
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), JsonError> {
        let data = input.read_value()?;
        match json::unmarshal::<BuildInfoFileId>(&data) {
            Err(_) => match json::unmarshal::<BuildInfoDiagnosticsOfFile>(&data) {
                Err(_) => Err(json::invalid("BuildInfoSemanticDiagnostic", &data)),
                Ok(diagnostics) => {
                    *self = Self {
                        file_id: 0,
                        diagnostics: Some(diagnostics),
                    };
                    Ok(())
                }
            },
            Ok(file_id) => {
                *self = Self {
                    file_id,
                    diagnostics: None,
                };
                Ok(())
            }
        }
    }
}

/// fileId if pending emit is same as what compilerOptions suggest
/// [fileId] if pending emit is only dts file emit
/// [fileId, emitKind] if any other type emit is pending
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildInfoFilePendingEmit {
    pub file_id: BuildInfoFileId,
    pub emit_kind: FileEmitKind,
}

impl Encode for BuildInfoFilePendingEmit {
    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfoFilePendingEmit.MarshalJSON
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), JsonError> {
        if self.emit_kind == FileEmitKind::NONE {
            return out.value(&self.file_id);
        }
        if self.emit_kind == FileEmitKind::DTS {
            let file_list_ids = vec![self.file_id];
            return out.value(&file_list_ids);
        }
        let file_and_emit_kind = vec![self.file_id, i64::from(self.emit_kind.0)];
        out.value(&file_and_emit_kind)
    }
}

impl Decode for BuildInfoFilePendingEmit {
    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfoFilePendingEmit.UnmarshalJSON
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), JsonError> {
        let data = input.read_value()?;
        match json::unmarshal::<BuildInfoFileId>(&data) {
            Err(_) => {
                let int_tuple = match json::unmarshal::<Vec<i64>>(&data) {
                    Ok(int_tuple) if !int_tuple.is_empty() => int_tuple,
                    _ => return Err(json::invalid("BuildInfoFilePendingEmit", &data)),
                };
                match int_tuple.len() {
                    1 => {
                        *self = Self {
                            file_id: int_tuple[0],
                            emit_kind: FileEmitKind::DTS,
                        };
                        Ok(())
                    }
                    2 => {
                        *self = Self {
                            file_id: int_tuple[0],
                            emit_kind: FileEmitKind(int_tuple[1] as u32),
                        };
                        Ok(())
                    }
                    length => Err(JsonError::Message(format!(
                        "invalid BuildInfoFilePendingEmit: expected 1 or 2 integers, got {length}"
                    ))),
                }
            }
            Ok(file_id) => {
                *self = Self {
                    file_id,
                    emit_kind: FileEmitKind::NONE,
                };
                Ok(())
            }
        }
    }
}

/// [fileId, signature] if different from file's signature
/// fileId if file wasnt emitted
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildInfoEmitSignature {
    pub file_id: BuildInfoFileId,
    /// Signature if it is different from file's Signature
    pub signature: JsString,
    /// true if signature is different only in dtsMap value
    pub differs_only_in_dts_map: bool,
    /// true if signature is different in options used to emit file
    pub differs_in_options: bool,
}

impl BuildInfoEmitSignature {
    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfoEmitSignature.noEmitSignature
    pub(crate) fn no_emit_signature(&self) -> bool {
        self.signature.is_empty() && !self.differs_only_in_dts_map && !self.differs_in_options
    }

    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfoEmitSignature.toEmitSignature
    pub(crate) fn to_emit_signature(
        &self,
        path: &Path,
        emit_signatures: &Mutex<HashMap<Path, EmitSignature>>,
    ) -> EmitSignature {
        let mut signature = JsString::default();
        let mut signature_with_different_options = None;
        if self.differs_only_in_dts_map {
            let mut list = Vec::with_capacity(1);
            let info = crate::snapshot::lock(emit_signatures).get(path).cloned();
            list.push(
                info.expect("the file's own signature is its default emit signature")
                    .signature,
            );
            signature_with_different_options = Some(list);
        } else if self.differs_in_options {
            signature_with_different_options = Some(vec![self.signature.clone()]);
        } else {
            signature.clone_from(&self.signature);
        }
        EmitSignature {
            signature,
            signature_with_different_options,
        }
    }
}

impl Encode for BuildInfoEmitSignature {
    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfoEmitSignature.MarshalJSON
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), JsonError> {
        if self.no_emit_signature() {
            return out.value(&self.file_id);
        }
        out.write_token(tsr_json::Token::BeginArray)?;
        out.value(&self.file_id)?;
        if self.differs_only_in_dts_map {
            out.value(&Vec::<JsString>::new())?;
        } else if self.differs_in_options {
            out.value(&vec![self.signature.clone()])?;
        } else {
            out.value(&self.signature)?;
        }
        out.write_token(tsr_json::Token::EndArray)
    }
}

impl Decode for BuildInfoEmitSignature {
    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfoEmitSignature.UnmarshalJSON
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), JsonError> {
        let data = input.read_value()?;
        let Err(_) = json::unmarshal::<BuildInfoFileId>(&data).map(|file_id| {
            *self = Self {
                file_id,
                ..Self::default()
            };
        }) else {
            return Ok(());
        };
        let Ok(file_id_and_signature) = json::unmarshal::<Vec<AnyValue>>(&data) else {
            return Err(json::invalid("BuildInfoEmitSignature", &data));
        };
        if file_id_and_signature.len() != 2 {
            return Err(JsonError::Message(format!(
                "invalid BuildInfoEmitSignature: expected 2 elements, got {}",
                file_id_and_signature.len()
            )));
        }
        let tsr_tsoptions::ConfigValue::Number(id) = file_id_and_signature[0].0 else {
            return Err(JsonError::Message(format!(
                "invalid fileId in BuildInfoEmitSignature: expected float64, got {}",
                json::go_type_name(&file_id_and_signature[0].0)
            )));
        };
        let file_id = id as BuildInfoFileId;
        let mut signature = JsString::default();
        let mut differs_only_in_dts_map = false;
        let mut differs_in_options = false;
        match &file_id_and_signature[1].0 {
            tsr_tsoptions::ConfigValue::String(signature_v) => signature = signature_v.clone(),
            tsr_tsoptions::ConfigValue::Array(signature_list) => {
                let signature_list = signature_list.as_deref().unwrap_or_default();
                match signature_list.len() {
                    0 => differs_only_in_dts_map = true,
                    1 => {
                        let tsr_tsoptions::ConfigValue::String(sig) = &signature_list[0] else {
                            return Err(JsonError::Message(format!(
                                "invalid signature in BuildInfoEmitSignature: expected string, got {}",
                                json::go_type_name(&signature_list[0])
                            )));
                        };
                        signature = sig.clone();
                        differs_in_options = true;
                    }
                    length => {
                        return Err(JsonError::Message(format!(
                            "invalid signature in BuildInfoEmitSignature: expected string or []string with 0 or 1 element, got {length} elements"
                        )))
                    }
                }
            }
            other => {
                return Err(JsonError::Message(format!(
                "invalid signature in BuildInfoEmitSignature: expected string or []string, got {}",
                json::go_type_name(other)
            )))
            }
        }
        *self = Self {
            file_id,
            signature,
            differs_only_in_dts_map,
            differs_in_options,
        };
        Ok(())
    }
}

/// `BuildInfoResolvedRoot`: `[resolved, root]`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildInfoResolvedRoot {
    pub resolved: BuildInfoFileId,
    pub root: BuildInfoFileId,
}

impl Encode for BuildInfoResolvedRoot {
    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfoResolvedRoot.MarshalJSON
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), JsonError> {
        out.value(&[self.resolved, self.root][..])
    }
}

impl Decode for BuildInfoResolvedRoot {
    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfoResolvedRoot.UnmarshalJSON
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), JsonError> {
        let data = input.read_value()?;
        let Ok([resolved, root]) = json::unmarshal_pair(&data) else {
            return Err(json::invalid("BuildInfoResolvedRoot", &data));
        };
        *self = Self { resolved, root };
        Ok(())
    }
}

/// `BuildInfo`. A slice the pin leaves nil when it has no entries is a
/// `Vec` here that encodes only when it is not empty; the slices the pin can
/// leave empty but non-nil (`fileInfos`, `contentMapperIdentities` and the
/// non-incremental `root`) are `Option`s.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BuildInfo {
    pub version: JsString,

    // Common between incremental and tsc -b buildinfo for non incremental programs
    pub errors: bool,
    pub check_pending: bool,
    pub root: Option<Vec<BuildInfoRoot>>,
    pub package_jsons: Vec<JsString>,
    pub missing_package_jsons: Vec<JsString>,
    pub content_mapper_identities: Option<Vec<JsString>>,

    // IncrementalProgram info
    pub file_names: Vec<JsString>,
    pub file_infos: Option<Vec<BuildInfoFileInfo>>,
    pub file_ids_list: Vec<Vec<BuildInfoFileId>>,
    pub options: Option<OrderedMap<JsString, AnyValue>>,
    pub referenced_map: Vec<BuildInfoReferenceMapEntry>,
    pub semantic_diagnostics_per_file: Vec<BuildInfoSemanticDiagnostic>,
    /// A file whose cached emit diagnostics are empty is the pin's nil entry.
    pub emit_diagnostics_per_file: Vec<Option<BuildInfoDiagnosticsOfFile>>,
    pub change_file_set: Vec<BuildInfoFileId>,
    pub affected_files_pending_emit: Vec<BuildInfoFilePendingEmit>,
    /// Because this is only output file in the program, we dont need fileId to deduplicate name
    pub latest_changed_dts_file: JsString,
    pub emit_signatures: Vec<BuildInfoEmitSignature>,
    pub resolved_root: Vec<BuildInfoResolvedRoot>,

    // NonIncrementalProgram info
    pub semantic_errors: bool,
}

impl Encode for BuildInfo {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), JsonError> {
        let mut object = json::Object::begin(out)?;
        object.string_omitzero(b"version", &self.version)?;
        object.bool_omitzero(b"errors", self.errors)?;
        object.bool_omitzero(b"checkPending", self.check_pending)?;
        if let Some(root) = &self.root {
            object.field(b"root", root)?;
        }
        object.slice_omitzero(b"packageJsons", &self.package_jsons)?;
        object.slice_omitzero(b"missingPackageJsons", &self.missing_package_jsons)?;
        if let Some(identities) = &self.content_mapper_identities {
            object.field(b"contentMapperIdentities", identities)?;
        }
        object.slice_omitzero(b"fileNames", &self.file_names)?;
        if let Some(file_infos) = &self.file_infos {
            object.field(b"fileInfos", file_infos)?;
        }
        object.slice_omitzero(b"fileIdsList", &self.file_ids_list)?;
        if let Some(options) = &self.options {
            object.field(b"options", options)?;
        }
        object.slice_omitzero(b"referencedMap", &self.referenced_map)?;
        object.slice_omitzero(
            b"semanticDiagnosticsPerFile",
            &self.semantic_diagnostics_per_file,
        )?;
        object.slice_omitzero(b"emitDiagnosticsPerFile", &self.emit_diagnostics_per_file)?;
        object.slice_omitzero(b"changeFileSet", &self.change_file_set)?;
        object.slice_omitzero(
            b"affectedFilesPendingEmit",
            &self.affected_files_pending_emit,
        )?;
        object.string_omitzero(b"latestChangedDtsFile", &self.latest_changed_dts_file)?;
        object.slice_omitzero(b"emitSignatures", &self.emit_signatures)?;
        object.slice_omitzero(b"resolvedRoot", &self.resolved_root)?;
        object.bool_omitzero(b"semanticErrors", self.semantic_errors)?;
        object.end()
    }
}

impl Decode for BuildInfo {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), JsonError> {
        json::object(input, |name, input| match name {
            b"version" => input.value(&mut self.version),
            b"errors" => input.value(&mut self.errors),
            b"checkPending" => input.value(&mut self.check_pending),
            b"root" => input.value(&mut self.root),
            b"packageJsons" => input.value(&mut self.package_jsons),
            b"missingPackageJsons" => input.value(&mut self.missing_package_jsons),
            b"contentMapperIdentities" => input.value(&mut self.content_mapper_identities),
            b"fileNames" => input.value(&mut self.file_names),
            b"fileInfos" => input.value(&mut self.file_infos),
            b"fileIdsList" => input.value(&mut self.file_ids_list),
            b"options" => {
                if input.peek_kind() == Kind::Null {
                    input.read_token()?;
                    self.options = None;
                    return Ok(());
                }
                input.value(self.options.get_or_insert_with(OrderedMap::default))
            }
            b"referencedMap" => input.value(&mut self.referenced_map),
            b"semanticDiagnosticsPerFile" => input.value(&mut self.semantic_diagnostics_per_file),
            b"emitDiagnosticsPerFile" => input.value(&mut self.emit_diagnostics_per_file),
            b"changeFileSet" => input.value(&mut self.change_file_set),
            b"affectedFilesPendingEmit" => input.value(&mut self.affected_files_pending_emit),
            b"latestChangedDtsFile" => input.value(&mut self.latest_changed_dts_file),
            b"emitSignatures" => input.value(&mut self.emit_signatures),
            b"resolvedRoot" => input.value(&mut self.resolved_root),
            b"semanticErrors" => input.value(&mut self.semantic_errors),
            _ => input.skip_value(),
        })
    }
}

impl BuildInfo {
    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfo.IsValidVersion
    pub fn is_valid_version(&self) -> bool {
        self.version.as_bytes() == tsr_core::version().as_bytes()
    }

    /// Whether the content mapper identities recorded in this build info
    /// match `current` (as [`content_mapper_identities`] produces them).
    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfo.ContentMapperIdentitiesMatch
    pub fn content_mapper_identities_match(&self, current: Option<&[JsString]>) -> bool {
        self.content_mapper_identities
            .as_deref()
            .unwrap_or_default()
            == current.unwrap_or_default()
    }

    /// `this` is `None` for the pin's nil receiver.
    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfo.IsIncremental
    pub fn is_incremental(this: Option<&Self>) -> bool {
        this.is_some_and(|b| !b.file_names.is_empty())
    }

    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfo.fileName
    pub(crate) fn file_name(&self, file_id: BuildInfoFileId) -> JsString {
        if file_id < 1 || file_id > self.file_names.len() as i64 {
            return JsString::default();
        }
        self.file_names[(file_id - 1) as usize].clone()
    }

    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfo.fileInfo
    pub(crate) fn file_info(&self, file_id: BuildInfoFileId) -> Option<&BuildInfoFileInfo> {
        let file_infos = self.file_infos.as_deref().unwrap_or_default();
        if file_id < 1 || file_id > file_infos.len() as i64 {
            return None;
        }
        Some(&file_infos[(file_id - 1) as usize])
    }

    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfo.GetCompilerOptions
    pub fn get_compiler_options(&self, build_info_directory: &[u8]) -> CompilerOptions {
        let mut options = CompilerOptions::default();
        for (option, value) in self.options.iter().flat_map(OrderedMap::entries) {
            if !build_info_directory.is_empty() {
                if let Some(result) = tsr_tsoptions::convert_option_to_absolute_path(
                    option.as_bytes(),
                    &value.0,
                    tsr_tsoptions::compiler_option_name_map(),
                    build_info_directory,
                ) {
                    tsr_tsoptions::parse_compiler_options(option.as_bytes(), &result, &mut options);
                    continue;
                }
            }
            tsr_tsoptions::parse_compiler_options(option.as_bytes(), &value.0, &mut options);
        }
        options
    }

    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfo.IsEmitPending
    pub fn is_emit_pending(
        &self,
        resolved: &ParsedCommandLine,
        build_info_directory: &[u8],
    ) -> bool {
        // Some of the emit files like source map or dts etc are not yet done
        if !resolved.options.no_emit.is_true() || resolved.options.emit_declarations() {
            let mut pending_emit = get_pending_emit_kind_with_options(
                &resolved.options,
                &self.get_compiler_options(build_info_directory),
            );
            if resolved.options.no_emit.is_true() {
                pending_emit &= FileEmitKind::DTS_ERRORS;
            }
            return pending_emit != FileEmitKind::NONE;
        }
        false
    }

    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfo.GetPackageJsons
    pub fn get_package_jsons<'a>(
        &'a self,
        build_info_directory: &'a [u8],
    ) -> impl Iterator<Item = Vec<u8>> + 'a {
        get_normalized_paths(&self.package_jsons, build_info_directory)
    }

    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfo.GetMissingPackageJsons
    pub fn get_missing_package_jsons<'a>(
        &'a self,
        build_info_directory: &'a [u8],
    ) -> impl Iterator<Item = Vec<u8>> + 'a {
        get_normalized_paths(&self.missing_package_jsons, build_info_directory)
    }

    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfo.GetBuildInfoRootInfoReader
    pub fn get_build_info_root_info_reader(
        &self,
        build_info_directory: &[u8],
        compare_paths_options_use_case_sensitive_file_names: bool,
    ) -> BuildInfoRootInfoReader {
        let mut resolved_root_file_infos = HashMap::with_capacity(self.file_names.len());
        // Roots of the File
        let mut root_to_resolved: OrderedMap<Path, Path> =
            OrderedMap::with_capacity(self.file_names.len());
        let mut resolved_to_root: HashMap<Path, Path> =
            HashMap::with_capacity(self.resolved_root.len());
        let to_path = |file_name: &[u8]| {
            tsr_tspath::to_path(
                file_name,
                build_info_directory,
                compare_paths_options_use_case_sensitive_file_names,
            )
        };

        // Create map from resolvedRoot to Root
        for resolved in &self.resolved_root {
            let resolved_root = self.file_name(resolved.resolved);
            let root = self.file_name(resolved.root);
            if !resolved_root.is_empty() && !root.is_empty() {
                resolved_to_root
                    .insert(to_path(resolved_root.as_bytes()), to_path(root.as_bytes()));
            }
        }

        let mut add_root = |resolved_root: JsString, file_info: Option<&BuildInfoFileInfo>| {
            if resolved_root.is_empty() {
                return;
            }
            let resolved_root_path = to_path(resolved_root.as_bytes());
            if let Some(root_path) = resolved_to_root.get(&resolved_root_path) {
                root_to_resolved.insert(root_path.clone(), resolved_root_path.clone());
            } else {
                root_to_resolved.insert(resolved_root_path.clone(), resolved_root_path.clone());
            }
            if let Some(file_info) = file_info {
                resolved_root_file_infos.insert(resolved_root_path, file_info.clone());
            }
        };

        for root in self.root.iter().flatten() {
            if !root.non_incremental.is_empty() {
                add_root(root.non_incremental.clone(), None);
            } else if root.end == 0 {
                add_root(self.file_name(root.start), self.file_info(root.start));
            } else {
                for i in root.start..=root.end {
                    add_root(self.file_name(i), self.file_info(i));
                }
            }
        }

        BuildInfoRootInfoReader {
            resolved_root_file_infos,
            root_to_resolved,
        }
    }
}

/// ContentMapperIdentities returns the project's sorted mapper transform
/// identities. A `None` project means the compiler host has no configured
/// content mappers.
// port: tsc/internal/execute/incremental/buildInfo.go:ContentMapperIdentities
pub fn content_mapper_identities(
    project: Option<&dyn tsr_contentmapper::Project>,
) -> Result<Option<Vec<JsString>>, tsr_contentmapper::Error> {
    let Some(project) = project else {
        return Ok(None);
    };
    Ok(Some(
        project
            .identities()?
            .into_iter()
            .map(|identity| JsString::from_bytes(identity.into_bytes()))
            .collect(),
    ))
}

// port: tsc/internal/execute/incremental/buildInfo.go:IsBuildInfoFileNameDefaultLibrary
pub fn is_build_info_file_name_default_library(file_name: &[u8]) -> bool {
    !tsr_tspath::is_relative(file_name) && !tsr_tspath::path_is_absolute(file_name)
}

// port: tsc/internal/execute/incremental/buildInfo.go:getNormalizedPaths
fn get_normalized_paths<'a>(
    paths: &'a [JsString],
    build_info_directory: &'a [u8],
) -> impl Iterator<Item = Vec<u8>> + 'a {
    paths
        .iter()
        .map(move |path| tsr_tspath::absolute(path.as_bytes(), build_info_directory))
}

/// `BuildInfoRootInfoReader`: the roots a build info records.
#[derive(Clone, Debug, Default)]
pub struct BuildInfoRootInfoReader {
    resolved_root_file_infos: HashMap<Path, BuildInfoFileInfo>,
    root_to_resolved: OrderedMap<Path, Path>,
}

impl BuildInfoRootInfoReader {
    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfoRootInfoReader.GetBuildInfoFileInfo
    pub fn get_build_info_file_info(
        &self,
        input_file_path: &Path,
    ) -> (Option<&BuildInfoFileInfo>, Path) {
        if let Some(info) = self.resolved_root_file_infos.get(input_file_path) {
            return (Some(info), input_file_path.clone());
        }
        if let Some(resolved) = self.root_to_resolved.get(input_file_path) {
            return (
                self.resolved_root_file_infos.get(resolved),
                resolved.clone(),
            );
        }
        (None, JsString::default())
    }

    // port: tsc/internal/execute/incremental/buildInfo.go:BuildInfoRootInfoReader.Roots
    pub fn roots(&self) -> impl Iterator<Item = &Path> {
        self.root_to_resolved.keys()
    }
}
