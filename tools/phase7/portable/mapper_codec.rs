//! Shared mapper-value transport. Production plugin execution stays on the host;
//! the consumer's public Project proxy preserves compiler-visible observations.
use crate::compiler_api::mapper as m;
use crate::compiler_codec::{freeze_diagnostic, thaw_diagnostic};
use crate::compiler_protocol as p;
use std::sync::Arc;
use tsr_ast::{span_map, MappedDiagnosticDirective};
use tsr_jsstring::JsString;
use tsr_tsoptions::config_mappers::{ContentMapper, MapperManifest};

fn js(value: &[u8]) -> JsString {
    JsString::from_bytes(value)
}
fn bytes(values: &[JsString]) -> Vec<Vec<u8>> {
    values.iter().map(|v| v.as_bytes().to_vec()).collect()
}
fn strings(values: &[Vec<u8>]) -> Vec<JsString> {
    values.iter().map(|v| js(v)).collect()
}

pub fn freeze_mapper(mapper: &ContentMapper) -> Result<p::MapperSnapshot, String> {
    Ok(p::MapperSnapshot {
        contribution_id: mapper.contribution_id.as_bytes().to_vec(),
        package: mapper.package.as_bytes().to_vec(),
        extensions: bytes(&mapper.extensions),
        options: mapper.options.clone(),
        package_directory: mapper.package_directory.as_bytes().to_vec(),
        manifest: p::MapperManifest {
            name: mapper.manifest.name.as_bytes().to_vec(),
            version: mapper.manifest.version.as_bytes().to_vec(),
            exec: mapper.manifest.exec.as_deref().map(bytes),
            compiler_options: mapper.manifest.compiler_options.as_deref().map(bytes),
            dynamic_config: mapper.manifest.dynamic_config,
        },
    })
}
pub fn thaw_mapper(mapper: &p::MapperSnapshot) -> Result<ContentMapper, String> {
    Ok(ContentMapper {
        contribution_id: js(&mapper.contribution_id),
        package: js(&mapper.package),
        extensions: strings(&mapper.extensions),
        options: mapper.options.clone(),
        package_directory: js(&mapper.package_directory),
        manifest: MapperManifest {
            name: js(&mapper.manifest.name),
            version: js(&mapper.manifest.version),
            exec: mapper.manifest.exec.as_deref().map(strings),
            compiler_options: mapper.manifest.compiler_options.as_deref().map(strings),
            dynamic_config: mapper.manifest.dynamic_config,
        },
    })
}

pub fn freeze_option(value: &m::OptionDiagnostic) -> p::MapperOptionDiagnostic {
    p::MapperOptionDiagnostic {
        mapper: value.mapper,
        path: value
            .path
            .iter()
            .map(|v| p::MapperOptionPath {
                property: v.property.clone(),
                index: v.index,
                is_index: v.is_index,
            })
            .collect(),
        source: value.source.clone(),
        code: value.code,
        message_text: value.message_text.clone(),
    }
}
pub fn thaw_option(value: p::MapperOptionDiagnostic) -> m::OptionDiagnostic {
    m::OptionDiagnostic {
        mapper: value.mapper,
        path: value
            .path
            .into_iter()
            .map(|v| m::OptionPathSegment {
                property: v.property,
                index: v.index,
                is_index: v.is_index,
            })
            .collect(),
        source: value.source,
        code: value.code,
        message_text: value.message_text,
    }
}
pub fn freeze_directive(value: &MappedDiagnosticDirective) -> p::DiagnosticDirective {
    p::DiagnosticDirective {
        original_pos: value.original_range.pos(),
        original_end: value.original_range.end(),
        virtual_pos: value.virtual_range.pos(),
        virtual_end: value.virtual_range.end(),
        policy: value.policy,
        unused_code: value.unused_code,
        unused_message_text: value.unused_message_text.as_bytes().to_vec(),
        source: value.source.as_bytes().to_vec(),
    }
}
pub fn thaw_directive(value: p::DiagnosticDirective) -> MappedDiagnosticDirective {
    MappedDiagnosticDirective {
        original_range: tsr_core::TextRange::new(value.original_pos, value.original_end),
        virtual_range: tsr_core::TextRange::new(value.virtual_pos, value.virtual_end),
        policy: value.policy,
        unused_code: value.unused_code,
        unused_message_text: JsString::from_bytes(value.unused_message_text),
        source: JsString::from_bytes(value.source),
    }
}
fn freeze_map(value: Option<&span_map::SpanMap>) -> Result<Option<Vec<u8>>, String> {
    value
        .map(|value| value.marshal().map_err(|e| e.to_string()))
        .transpose()
}
fn thaw_map(value: Option<Vec<u8>>) -> Result<Option<Arc<span_map::SpanMap>>, String> {
    value
        .map(|bytes| {
            span_map::unmarshal(&bytes)
                .map(Arc::new)
                .map_err(|e| e.to_string())
        })
        .transpose()
}
pub fn freeze_transform(value: &m::TransformResult) -> Result<p::MapperTransform, String> {
    Ok(p::MapperTransform {
        text: value.text.clone(),
        virtual_extension: value.virtual_extension.clone(),
        diagnostics: value
            .diagnostics
            .iter()
            .map(|d| {
                freeze_diagnostic(
                    d,
                    &|_| Err("mapper-authored diagnostic unexpectedly has a source owner".into()),
                    &|d| Ok(d.message_args.clone()),
                )
            })
            .collect::<Result<_, _>>()?,
        mappings: freeze_map(value.mappings.as_deref())?,
        diagnostic_directives: value
            .diagnostic_directives
            .iter()
            .map(freeze_directive)
            .collect(),
        supplemental: value
            .supplemental
            .iter()
            .map(|v| {
                Ok(p::MapperMappedResult {
                    text: v.text.clone(),
                    virtual_extension: v.virtual_extension.clone(),
                    mappings: freeze_map(v.mappings.as_deref())?,
                    diagnostic_directives: v
                        .diagnostic_directives
                        .iter()
                        .map(freeze_directive)
                        .collect(),
                })
            })
            .collect::<Result<_, String>>()?,
    })
}
pub fn thaw_transform(value: p::MapperTransform) -> Result<m::TransformResult, String> {
    Ok(m::TransformResult {
        text: value.text,
        virtual_extension: value.virtual_extension,
        diagnostics: value
            .diagnostics
            .into_iter()
            .map(|d| {
                thaw_diagnostic(d, &|_| {
                    Err("mapper-authored diagnostic names a transported source owner".into())
                })
            })
            .collect::<Result<_, _>>()?,
        mappings: thaw_map(value.mappings)?,
        diagnostic_directives: value
            .diagnostic_directives
            .into_iter()
            .map(thaw_directive)
            .collect(),
        supplemental: value
            .supplemental
            .into_iter()
            .map(|v| {
                Ok(m::MappedResult {
                    text: v.text,
                    virtual_extension: v.virtual_extension,
                    mappings: thaw_map(v.mappings)?,
                    diagnostic_directives: v
                        .diagnostic_directives
                        .into_iter()
                        .map(thaw_directive)
                        .collect(),
                })
            })
            .collect::<Result<_, String>>()?,
    })
}

pub fn freeze_error(value: &m::Error) -> p::MapperError {
    use p::MapperError as E;
    match value {
        m::Error::Transform(stage, cause) => E::Transform {
            stage: format!("{stage:?}"),
            cause: cause.as_deref().map(|v| Box::new(freeze_error(v))),
        },
        m::Error::DiagnosticDirective {
            kind,
            index,
            supplemental_index,
            policy,
        } => E::DiagnosticDirective {
            directive_kind: format!("{kind:?}"),
            index: *index,
            supplemental_index: *supplemental_index,
            policy: *policy,
        },
        m::Error::InvalidVirtualExtension(extension) => E::InvalidVirtualExtension {
            extension: extension.clone(),
        },
        m::Error::Project(kind) => E::Project {
            project_kind: format!("{kind:?}"),
        },
        m::Error::Initialize(value) => E::Initialize {
            initialize_kind: format!("{:?}", value.kind),
            mapper_name: value.mapper_name.as_bytes().to_vec(),
            command: value.command.as_bytes().to_vec(),
            detail: value.detail.clone(),
            exit_code: value.exit_code,
            timeout_seconds: value.timeout_seconds,
            position_encoding: value.position_encoding.as_str().to_owned(),
            diagnostic_source: value.diagnostic_source.clone(),
        },
        m::Error::SupplementalFileCollision(name) => E::SupplementalFileCollision {
            name: name.as_bytes().to_vec(),
        },
        m::Error::Mapping(value) => E::Mapping {
            mapping_kind: format!("{:?}", value.kind),
            virtual_pos: value.virtual_pos,
            original_pos: value.original_pos,
        },
        m::Error::Ipc(value) => E::Ipc {
            text: value.to_string(),
        },
        m::Error::Json(value) => E::Json {
            text: value.to_string(),
        },
        m::Error::SpanMap(span_map::UnmarshalError::Json(value)) => E::SpanMapJson {
            text: value.to_string(),
        },
        m::Error::SpanMap(span_map::UnmarshalError::Arity { segment, values }) => E::SpanMapArity {
            segment: *segment,
            values: *values,
        },
        m::Error::Message(text) => E::Message { text: text.clone() },
    }
}

macro_rules! decode_kind {
    ($value:expr, $ty:ident, [$($variant:ident),+ $(,)?]) => {{
        match $value.as_str() {
            $(stringify!($variant) => $ty::$variant,)+
            other => return Err(format!("unknown {}: {other}", stringify!($ty))),
        }
    }};
}
pub fn thaw_error(value: p::MapperError) -> Result<m::Error, String> {
    use m::{
        DiagnosticDirectiveErrorKind, InitializeErrorKind, ProjectErrorKind, TransformErrorKind,
    };
    use p::MapperError as E;
    use span_map::MappingErrorKind;
    Ok(match value {
        E::Transform { stage, cause } => m::Error::Transform(
            decode_kind!(
                stage,
                TransformErrorKind,
                [Unknown, Initialize, Project, Request, Response, Mappings]
            ),
            cause.map(|v| thaw_error(*v).map(Box::new)).transpose()?,
        ),
        E::DiagnosticDirective {
            directive_kind,
            index,
            supplemental_index,
            policy,
        } => m::Error::DiagnosticDirective {
            kind: decode_kind!(
                directive_kind,
                DiagnosticDirectiveErrorKind,
                [
                    InvalidRange,
                    InvalidPolicy,
                    ExpectMissingUnusedDiagnostic,
                    InvalidUnusedDiagnosticIndex,
                    Overlap
                ]
            ),
            index,
            supplemental_index,
            policy,
        },
        E::InvalidVirtualExtension { extension } => m::Error::InvalidVirtualExtension(extension),
        E::Project { project_kind } => m::Error::Project(decode_kind!(
            project_kind,
            ProjectErrorKind,
            [
                MalformedResponse,
                MissingConfigIdentity,
                NonAbsoluteWatchedFile,
                UnexpectedConfigIdentity,
                UnexpectedWatchedFiles
            ]
        )),
        E::Initialize {
            initialize_kind,
            mapper_name,
            command,
            detail,
            exit_code,
            timeout_seconds,
            position_encoding,
            diagnostic_source,
        } => m::Error::Initialize(Box::new(m::InitializeError {
            kind: decode_kind!(
                initialize_kind,
                InitializeErrorKind,
                [
                    ProcessStart,
                    ProcessExit,
                    NoResponse,
                    InvalidResponse,
                    Request,
                    PositionEncoding,
                    EmptyDiagnosticSource,
                    ReservedDiagnosticSource
                ]
            ),
            mapper_name: js(&mapper_name),
            command: js(&command),
            detail,
            exit_code,
            timeout_seconds,
            position_encoding: m::PositionEncoding(position_encoding),
            diagnostic_source,
        })),
        E::SupplementalFileCollision { name } => m::Error::SupplementalFileCollision(js(&name)),
        E::Mapping {
            mapping_kind,
            virtual_pos,
            original_pos,
        } => m::Error::Mapping(span_map::MappingError {
            kind: decode_kind!(
                mapping_kind,
                MappingErrorKind,
                [Overlap, OutOfBounds, VerbatimMismatch, Kind, Feature]
            ),
            virtual_pos,
            original_pos,
        }),
        E::Ipc { text } => m::Error::Ipc(m::IpcError::Message(text)),
        E::Json { text } => m::Error::Json(tsr_json::Error::Message(text)),
        E::SpanMapJson { text } => m::Error::SpanMap(span_map::UnmarshalError::Json(
            tsr_json::Error::Message(text),
        )),
        E::SpanMapArity { segment, values } => {
            m::Error::SpanMap(span_map::UnmarshalError::Arity { segment, values })
        }
        E::Message { text } => m::Error::Message(text),
    })
}
