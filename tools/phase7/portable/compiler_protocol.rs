//! Compiler-independent values for persistent public-session acceptance.
//! Byte strings and source indices cross the wire; AST/checker identities do not.
use super::protocol::DiagnosticRecord;
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;
pub type CompilationId = u64;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompilerRequest {
    pub version: u32,
    pub command: CompilerCommand,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum CompilerCommand {
    Compile {
        id: CompilationId,
        input: Box<CompilationInput>,
    },
    ObserveTypesAndSymbols {
        id: CompilationId,
        files: Vec<Vec<u8>>,
        had_errors: bool,
    },
    Assertions {
        id: CompilationId,
        union_ordering: bool,
        parent_pointers: bool,
    },
    Release {
        id: CompilationId,
    },
}
impl CompilerCommand {
    pub fn id(&self) -> CompilationId {
        match self {
            Self::Compile { id, .. }
            | Self::ObserveTypesAndSymbols { id, .. }
            | Self::Assertions { id, .. }
            | Self::Release { id } => *id,
        }
    }
}

/// Host preparation, after CompileFilesEx has finalized paths and selected
/// library fixtures. Files before `symlinks_after` precede symlinks; the
/// remaining physical library files follow them, matching native insertion.
/// Bundled library files are explicit: no consumer filesystem fallback exists.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompilationInput {
    pub compiler_options: Vec<u8>,
    pub root_names: Vec<Vec<u8>>,
    pub files: Vec<PhysicalFile>,
    pub symlinks: Vec<Symlink>,
    pub symlinks_after: usize,
    pub current_directory: Vec<u8>,
    pub default_library_path: Vec<u8>,
    pub case_sensitive: bool,
    pub single_threaded: i8,
    pub capture_suggestions: bool,
    pub config: ConfigSnapshot,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhysicalFile {
    pub name: Vec<u8>,
    pub content: Vec<u8>,
    pub loaded: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Symlink {
    pub name: Vec<u8>,
    pub target: Vec<u8>,
}

/// Exactly the configuration fields copied by native CompileFilesEx.
/// Diagnostic file indices here refer to `sources`, not response sources.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigSnapshot {
    pub sources: Vec<ConfigSource>,
    pub main: Option<usize>,
    pub dependencies: Vec<usize>,
    pub errors: Vec<DiagnosticRecord>,
    pub specs: Option<ConfigSpecs>,
    pub mappers: Option<Vec<MapperSnapshot>>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigSource {
    pub name: Vec<u8>,
    pub path: Vec<u8>,
    pub text: Vec<u8>,
    pub extended_source_files: Vec<Vec<u8>>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigSpecs {
    pub files_specs: ConfigValueSnapshot,
    pub include_specs: ConfigValueSnapshot,
    pub exclude_specs: ConfigValueSnapshot,
    pub validated_files: Vec<Vec<u8>>,
    pub validated_includes: Vec<Vec<u8>>,
    pub validated_excludes: Vec<Vec<u8>>,
    pub files_before_substitution: Vec<Vec<u8>>,
    pub includes_before_substitution: Vec<Vec<u8>>,
    pub is_default_include: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ConfigValueSnapshot {
    Null,
    EmptyStruct,
    Boolean(bool),
    Number(f64),
    Integer(i64),
    Enum(i32),
    String(Vec<u8>),
    Array(Option<Vec<Self>>),
    Object(Vec<(Vec<u8>, Self)>),
    StringArray(Option<Vec<Vec<u8>>>),
    UnorderedObject(Vec<(Vec<u8>, Self)>),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapperSnapshot {
    pub contribution_id: Vec<u8>,
    pub package: Vec<u8>,
    pub extensions: Vec<Vec<u8>>,
    pub options: Option<Vec<u8>>,
    pub manifest: MapperManifest,
    pub package_directory: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapperManifest {
    pub name: Vec<u8>,
    pub version: Vec<u8>,
    pub exec: Option<Vec<Vec<u8>>>,
    pub compiler_options: Option<Vec<Vec<u8>>>,
    pub dynamic_config: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompilerResponse {
    pub version: u32,
    pub id: CompilationId,
    pub reply: CompilerReply,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "result",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum CompilerReply {
    Compiled(Box<CompilationOutput>),
    Observed(ObservationOutput),
    Assertions(AssertionOutput),
    Released,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompilationOutput {
    pub sources: Vec<SourceSnapshot>,
    pub pre_files: Vec<usize>,
    pub post_files: Vec<usize>,
    pub pre_diagnostics: Vec<DiagnosticRecord>,
    pub post_diagnostics: Vec<DiagnosticRecord>,
    pub emit: Option<EmitSnapshot>,
    pub facts: ProgramFacts,
    pub trace: Vec<ResolutionTrace>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSnapshot {
    pub name: Vec<u8>,
    pub path: Vec<u8>,
    pub text: Vec<u8>,
    pub script_kind: i32,
    pub is_declaration_file: bool,
    pub mapped: Option<MappedSource>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MappedSource {
    pub content_mapper: Vec<u8>,
    pub transform_identity: Vec<u8>,
    pub virtual_file_name: Vec<u8>,
    pub original_text: Vec<u8>,
    pub span_map: Option<Vec<u8>>,
    pub canonical_source: Option<usize>,
    pub canonical_name: Option<Vec<u8>>,
    pub supplemental_names: Vec<Vec<u8>>,
    pub supplemental_sources: Vec<usize>,
    pub diagnostic_directives: Vec<DiagnosticDirective>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticDirective {
    pub original_pos: i64,
    pub original_end: i64,
    pub virtual_pos: i64,
    pub virtual_end: i64,
    pub policy: u8,
    pub unused_code: i32,
    pub unused_message_text: Vec<u8>,
    pub source: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProgramFacts {
    pub root_names: Vec<Vec<u8>>,
    pub compiler_options: Vec<u8>,
    pub current_directory: Vec<u8>,
    pub case_sensitive: bool,
    pub common_source_directory: Result<Vec<u8>, String>,
    pub content_mapper_extensions: Vec<Vec<u8>>,
    /// Canonical query path -> global response source index. A missing query
    /// is explicit and is never replaced by a host parse or lookup guess.
    pub lookups: Vec<(Vec<u8>, Option<usize>)>,
    pub default_library_sources: Vec<usize>,
    pub mapped_files: Vec<(usize, Vec<Vec<u8>>)>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmitSnapshot {
    pub emit_skipped: bool,
    pub diagnostics: Vec<DiagnosticRecord>,
    /// Output bytes live once, in emitted-files order followed by any unlisted
    /// successful callback writes. `emitted_files` and writes refer here.
    pub files: Vec<PhysicalFile>,
    pub emitted_files: Vec<usize>,
    pub writes: Vec<WriteSnapshot>,
    pub source_maps: Vec<SourceMapSnapshot>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteSnapshot {
    pub file: usize,
    pub real_name: Vec<u8>,
    pub canonical_real_name: Vec<u8>,
    pub source: Option<usize>,
    pub source_map_url_pos: isize,
    pub diagnostics: Vec<DiagnosticRecord>,
    pub skipped_dts_write: bool,
    /// Exact production BuildInfo JSON when metadata carries it.
    pub build_info: Option<Vec<u8>>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceMapSnapshot {
    pub input_source_file_names: Vec<Vec<u8>>,
    pub generated_file: Vec<u8>,
    pub source_map: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolutionTrace {
    pub message_key: Vec<u8>,
    pub args: Vec<TraceArgument>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum TraceArgument {
    Text(Vec<u8>),
    Bool(bool),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationOutput {
    pub types_error: Option<String>,
    pub symbols_error: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssertionOutput {
    pub union_ordering: Option<serde_json::Value>,
    pub parent_pointers: Option<serde_json::Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum CompilerOutputFrame {
    Response(CompilerResponse),
    Callback(CompilerCallback),
    Failure(String),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompilerCallback {
    pub id: CompilationId,
    pub call: CallbackCall,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "call", rename_all = "snake_case", deny_unknown_fields)]
pub enum CallbackCall {
    Mapper {
        request: MapperCall,
    },
    ObservationBatch {
        symbols: bool,
        file_index: usize,
        rows: Vec<ObservationRow>,
    },
}
// The shared collector owns these values. Including this pure DTO source here
// lets host protocols deserialize observations without importing a checker.
#[path = "walker/rows.rs"]
pub mod observation_rows;
pub use observation_rows::Row as ObservationRow;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "reply",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum CompilerCallbackReply {
    Mapper(MapperReply),
    Continue,
    StopDomain,
    Failure(String),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum MapperCall {
    Refresh,
    Identities,
    Identity {
        mapper: usize,
    },
    WatchedFiles,
    Diagnostics,
    Transform {
        mapper: usize,
        file_name: Vec<u8>,
        content: Vec<u8>,
    },
    Close,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "result",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum MapperReply {
    Unit,
    Strings(Vec<String>),
    String(String),
    Diagnostics(Vec<MapperOptionDiagnostic>),
    Transform(MapperTransform),
    Error(MapperError),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapperOptionDiagnostic {
    pub mapper: usize,
    pub path: Vec<MapperOptionPath>,
    pub source: String,
    pub code: i32,
    pub message_text: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapperOptionPath {
    pub property: String,
    pub index: usize,
    pub is_index: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapperTransform {
    pub text: String,
    pub virtual_extension: String,
    pub diagnostics: Vec<DiagnosticRecord>,
    pub mappings: Option<Vec<u8>>,
    pub diagnostic_directives: Vec<DiagnosticDirective>,
    pub supplemental: Vec<MapperMappedResult>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapperMappedResult {
    pub text: String,
    pub virtual_extension: String,
    pub mappings: Option<Vec<u8>>,
    pub diagnostic_directives: Vec<DiagnosticDirective>,
}
/// Compiler-observable mapper error structure. Foreign IPC/JSON causes retain
/// their exact display text inside the original outer error class; the compiler
/// inspects mapper stages and initialize/mapping fields, not foreign internals.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MapperError {
    Transform {
        stage: String,
        cause: Option<Box<Self>>,
    },
    DiagnosticDirective {
        directive_kind: String,
        index: usize,
        supplemental_index: Option<usize>,
        policy: u8,
    },
    InvalidVirtualExtension {
        extension: String,
    },
    Project {
        project_kind: String,
    },
    Initialize {
        initialize_kind: String,
        mapper_name: Vec<u8>,
        command: Vec<u8>,
        detail: String,
        exit_code: i32,
        timeout_seconds: i32,
        position_encoding: String,
        diagnostic_source: String,
    },
    SupplementalFileCollision {
        name: Vec<u8>,
    },
    Mapping {
        mapping_kind: String,
        virtual_pos: i32,
        original_pos: i32,
    },
    Ipc {
        text: String,
    },
    Json {
        text: String,
    },
    SpanMapJson {
        text: String,
    },
    SpanMapArity {
        segment: usize,
        values: usize,
    },
    Message {
        text: String,
    },
}
