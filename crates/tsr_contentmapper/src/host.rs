//! The host's contract (`host.go`): its errors, the transform result, the
//! project a program transforms through, and the timing snapshot.
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tsr_ast::span_map::SpanMap;
use tsr_ast::{Diagnostic, MappedDiagnosticDirective};
use tsr_core::CompilerOptions;
use tsr_jsstring::JsString;
use tsr_tsoptions::config_mappers::ContentMapper;

use crate::protocol::{DiagnosticDirectivePolicy, PositionEncoding};

/// The stage at which a transform failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransformErrorKind {
    Unknown,
    Initialize,
    Project,
    Request,
    Response,
    Mappings,
}

/// Why a diagnostic directive was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticDirectiveErrorKind {
    InvalidRange,
    InvalidPolicy,
    ExpectMissingUnusedDiagnostic,
    InvalidUnusedDiagnosticIndex,
    Overlap,
}

/// Why an `openProject` response was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectErrorKind {
    MalformedResponse,
    MissingConfigIdentity,
    NonAbsoluteWatchedFile,
    UnexpectedConfigIdentity,
    UnexpectedWatchedFiles,
}

/// Why a mapper's initialization failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitializeErrorKind {
    ProcessStart,
    ProcessExit,
    NoResponse,
    InvalidResponse,
    Request,
    PositionEncoding,
    EmptyDiagnosticSource,
    ReservedDiagnosticSource,
}

/// An invalid or unsupported mapper initialization.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InitializeError {
    pub kind: InitializeErrorKind,
    pub mapper_name: JsString,
    pub command: JsString,
    pub detail: String,
    pub exit_code: i32,
    pub timeout_seconds: i32,
    pub position_encoding: PositionEncoding,
    pub diagnostic_source: String,
}

impl InitializeError {
    pub fn new(kind: InitializeErrorKind) -> Self {
        Self {
            kind,
            mapper_name: JsString::default(),
            command: JsString::default(),
            detail: String::new(),
            exit_code: 0,
            timeout_seconds: 0,
            position_encoding: PositionEncoding::default(),
            diagnostic_source: String::new(),
        }
    }
}

/// A content-mapper failure: the pinned error types and the transport's.
#[derive(Clone, Debug)]
pub enum Error {
    /// `TransformError`: a stage and its cause.
    Transform(TransformErrorKind, Option<Box<Error>>),
    /// `DiagnosticDirectiveError`.
    DiagnosticDirective {
        kind: DiagnosticDirectiveErrorKind,
        index: usize,
        /// `-1` in the pin when the directive is in the canonical output.
        supplemental_index: Option<usize>,
        policy: DiagnosticDirectivePolicy,
    },
    /// `InvalidVirtualExtensionError`.
    InvalidVirtualExtension(String),
    /// `ProjectError`.
    Project(ProjectErrorKind),
    /// `InitializeError`.
    Initialize(Box<InitializeError>),
    /// `SupplementalFileCollisionError`.
    SupplementalFileCollision(JsString),
    /// `*spanmap.MappingError`.
    Mapping(tsr_ast::span_map::MappingError),
    /// A connection failure.
    Ipc(tsr_ipc::Error),
    Json(tsr_json::Error),
    SpanMap(tsr_ast::span_map::UnmarshalError),
    /// Any other error text.
    Message(String),
}

impl Error {
    /// `errors.AsType[*TransformError]`: the outermost transform error.
    pub fn transform_kind(&self) -> Option<TransformErrorKind> {
        match self {
            Self::Transform(kind, _) => Some(*kind),
            _ => None,
        }
    }
    /// `errors.AsType[*InitializeError]`, through transform errors.
    pub fn initialize_error(&self) -> Option<&InitializeError> {
        match self {
            Self::Initialize(error) => Some(error),
            Self::Transform(_, Some(cause)) => cause.initialize_error(),
            _ => None,
        }
    }
    /// `errors.AsType[*ProjectError]`, through transform errors.
    pub fn project_error(&self) -> Option<ProjectErrorKind> {
        match self {
            Self::Project(kind) => Some(*kind),
            Self::Transform(_, Some(cause)) => cause.project_error(),
            _ => None,
        }
    }
    /// The innermost cause of a transform error, for the typed lookups the
    /// diagnostics make (`errors.AsType` walks the chain).
    /// port: tsc/internal/contentmapper/host.go:TransformError.Unwrap
    pub fn cause(&self) -> &Self {
        match self {
            Self::Transform(_, Some(cause)) => cause.cause(),
            error => error,
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // port: tsc/internal/contentmapper/host.go:TransformError.Error
            Self::Transform(_, cause) => match cause {
                Some(cause) => write!(f, "content mapper transform failed: {cause}"),
                None => f.write_str("content mapper transform failed: <nil>"),
            },
            // port: tsc/internal/contentmapper/host.go:DiagnosticDirectiveError.Error
            Self::DiagnosticDirective { index, .. } => {
                write!(f, "invalid content mapper diagnostic directive {index}")
            }
            // port: tsc/internal/contentmapper/host.go:InvalidVirtualExtensionError.Error
            Self::InvalidVirtualExtension(extension) => write!(
                f,
                "invalid virtual extension {}",
                tsr_jsstring::go_quote(extension.as_bytes())
            ),
            // port: tsc/internal/contentmapper/host.go:ProjectError.Error
            Self::Project(kind) => f.write_str(match kind {
                ProjectErrorKind::MalformedResponse => {
                    "content mapper returned a malformed project response"
                }
                ProjectErrorKind::MissingConfigIdentity => {
                    "content mapper did not return configIdentity for dynamic configuration"
                }
                ProjectErrorKind::NonAbsoluteWatchedFile => {
                    "content mapper returned a non-absolute path in watchedFiles"
                }
                ProjectErrorKind::UnexpectedConfigIdentity => {
                    "content mapper returned configIdentity without declaring dynamicConfig"
                }
                ProjectErrorKind::UnexpectedWatchedFiles => {
                    "content mapper returned watchedFiles without declaring dynamicConfig"
                }
            }),
            Self::Initialize(error) => write!(f, "{}", initialize_error_text(error)),
            // port: tsc/internal/contentmapper/host.go:SupplementalFileCollisionError.Error
            Self::SupplementalFileCollision(name) => write!(
                f,
                "content mapper supplemental output file {} already exists",
                tsr_jsstring::go_quote(name.as_bytes())
            ),
            Self::Mapping(error) => write!(f, "{error}"),
            Self::Ipc(error) => write!(f, "{error}"),
            Self::Json(error) => write!(f, "{error}"),
            Self::SpanMap(error) => write!(f, "{error}"),
            Self::Message(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

/// port: tsc/internal/contentmapper/host.go:InitializeError.Error
fn initialize_error_text(error: &InitializeError) -> String {
    let quote = |bytes: &[u8]| tsr_jsstring::go_quote(bytes);
    match error.kind {
        InitializeErrorKind::ProcessStart => format!(
            "could not start content mapper command {}: {}",
            quote(error.command.as_bytes()),
            error.detail
        ),
        InitializeErrorKind::ProcessExit => format!(
            "content mapper process exited before initialization with code {}",
            error.exit_code
        ),
        InitializeErrorKind::NoResponse => {
            "content mapper did not respond to the initialize request".into()
        }
        InitializeErrorKind::InvalidResponse => {
            format!(
                "content mapper returned an invalid initialize response: {}",
                error.detail
            )
        }
        InitializeErrorKind::Request => {
            format!("content mapper initialize request failed: {}", error.detail)
        }
        InitializeErrorKind::PositionEncoding => format!(
            "unsupported position encoding {}",
            quote(error.position_encoding.as_str().as_bytes())
        ),
        InitializeErrorKind::EmptyDiagnosticSource => "diagnostic source must not be empty".into(),
        InitializeErrorKind::ReservedDiagnosticSource => format!(
            "diagnostic source {} is reserved by TypeScript",
            quote(error.diagnostic_source.as_bytes())
        ),
    }
}

impl From<tsr_ipc::Error> for Error {
    fn from(error: tsr_ipc::Error) -> Self {
        Self::Ipc(error)
    }
}

impl From<tsr_json::Error> for Error {
    fn from(error: tsr_json::Error) -> Self {
        Self::Json(error)
    }
}

/// A transform error of `kind` caused by `error`.
/// port: tsc/internal/contentmapper/host.go:NewTransformError
pub fn transform_error(kind: TransformErrorKind, error: Option<Error>) -> Error {
    Error::Transform(kind, error.map(Box::new))
}

/// The outcome of transforming a content-mapped file into virtual source.
#[derive(Clone, Debug, Default)]
pub struct TransformResult {
    pub text: String,
    pub virtual_extension: String,
    /// Mapper-authored errors in original coordinates, without a file yet.
    pub diagnostics: Vec<Diagnostic>,
    /// A successful transform always carries a map; an empty one describes
    /// fully synthesized output.
    pub mappings: Option<Arc<SpanMap>>,
    pub diagnostic_directives: Vec<MappedDiagnosticDirective>,
    pub supplemental: Vec<MappedResult>,
}

/// One virtual source file and its mapping to the original input.
#[derive(Clone, Debug, Default)]
pub struct MappedResult {
    pub text: String,
    pub virtual_extension: String,
    pub mappings: Option<Arc<SpanMap>>,
    pub diagnostic_directives: Vec<MappedDiagnosticDirective>,
}

/// The inputs for transforming one content-mapped file.
#[derive(Clone, Debug, Default)]
pub struct Request {
    pub file_name: JsString,
    pub content: Vec<u8>,
}

/// The project configuration visible to its mappers. The pin keys shared
/// projects by the identity of its options and mapper pointers; here the
/// mappers are one shared list, and a mapper is its index in it.
#[derive(Clone, Debug, Default)]
pub struct ProjectSpec {
    pub config_file_name: JsString,
    pub mappers: Arc<[ContentMapper]>,
    pub compiler_options: Arc<CompilerOptions>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptionPathSegment {
    pub property: String,
    pub index: usize,
    pub is_index: bool,
}

/// A mapper's report on its entry's options.
#[derive(Clone, Debug)]
pub struct OptionDiagnostic {
    /// The mapper's index in its project's list.
    pub mapper: usize,
    pub path: Vec<OptionPathSegment>,
    pub source: String,
    pub code: i32,
    pub message_text: String,
}

/// Cumulative wall time and invocations of one mapper operation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OperationTiming {
    pub count: u64,
    pub duration: Duration,
}

/// Cumulative process and protocol activity of one mapper identity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MapperTimings {
    pub spawn: OperationTiming,
    pub initialize: OperationTiming,
    pub open_project: OperationTiming,
    pub close_project: OperationTiming,
    pub transform: OperationTiming,
}

/// A cumulative snapshot of the host's mapper activity.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Timings {
    pub mappers: HashMap<String, MapperTimings>,
    pub request_wait: Duration,
}

impl Timings {
    /// The non-negative delta since `previous`.
    /// port: tsc/internal/contentmapper/host.go:Timings.Since
    #[must_use]
    pub fn since(&self, previous: &Self) -> Self {
        Self {
            mappers: self
                .mappers
                .iter()
                .map(|(identity, current)| {
                    let before = previous.mappers.get(identity).copied().unwrap_or_default();
                    (
                        identity.clone(),
                        MapperTimings {
                            spawn: operation_timing_since(current.spawn, before.spawn),
                            initialize: operation_timing_since(
                                current.initialize,
                                before.initialize,
                            ),
                            open_project: operation_timing_since(
                                current.open_project,
                                before.open_project,
                            ),
                            close_project: operation_timing_since(
                                current.close_project,
                                before.close_project,
                            ),
                            transform: operation_timing_since(current.transform, before.transform),
                        },
                    )
                })
                .collect(),
            request_wait: self.request_wait.saturating_sub(previous.request_wait),
        }
    }
}

/// port: tsc/internal/contentmapper/host.go:operationTimingSince
fn operation_timing_since(current: OperationTiming, previous: OperationTiming) -> OperationTiming {
    OperationTiming {
        count: current.count - current.count.min(previous.count),
        duration: current.duration.saturating_sub(previous.duration),
    }
}

/// The project-scoped view of a host: mapper configuration handles, the
/// identities for caching, and the transforms. Mappers are indices into the
/// project's spec.
pub trait Project: Send + Sync {
    /// Closes opened mapper projects so they reopen on the next use.
    fn refresh(&self) -> Result<(), Error>;
    /// Sorted transform identities of every configured mapper.
    fn identities(&self) -> Result<Vec<String>, Error>;
    /// The transform identity of `mapper`, or empty when it is not in the project.
    fn identity(&self, mapper: usize) -> Result<String, Error>;
    /// Files mappers with dynamic configuration watch.
    fn watched_files(&self) -> Result<Vec<String>, Error>;
    /// Option diagnostics of the mapper projects already opened.
    fn diagnostics(&self) -> Vec<OptionDiagnostic>;
    /// Transforms one content-mapped file with `mapper`.
    fn transform(&self, mapper: usize, request: &Request) -> Result<TransformResult, Error>;
    /// Releases this project reference.
    fn close(&self) -> Result<(), Error>;
}

/// Drives the configured content mappers during program construction.
pub trait Host: Send + Sync {
    /// A cumulative snapshot of mapper activity.
    fn timings(&self) -> Timings;
    /// A retained project for `spec`; the caller closes it.
    fn project(&self, spec: ProjectSpec) -> Option<Arc<dyn Project>>;
    /// Retains the processes of these mappers until the returned release runs.
    fn acquire(&self, mappers: &[ContentMapper]) -> Box<dyn FnOnce() + Send>;
    /// Uses `locale` for mapper-authored messages from now on.
    fn set_locale(&self, locale: tsr_locale::Locale);
    /// Transforms one file in a short-lived project with default options.
    fn transform(
        &self,
        mapper: &ContentMapper,
        request: &Request,
    ) -> Result<TransformResult, Error>;
    /// Shuts down every mapper the host started.
    fn close(&self) -> Result<(), Error>;
}
