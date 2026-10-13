//! Rust embedding APIs. Disable default features for the parser-only surface.
//!
//! Loaded source preserves its original bytes. The host supplies canonical
//! source-file options. The checker feature adds an immutable program session
//! with scoped queries, explicit retained handles, an in-memory emit and
//! single-file JavaScript and declaration transpilation.

#[cfg(feature = "checker")]
pub mod content_mapper;
#[cfg(feature = "checker")]
mod session;
#[cfg(feature = "checker")]
mod transpile;
#[cfg(feature = "checker")]
pub use session::{
    build_info_reader, CompilerOperations, DiagnosticKind, EmitOnly, EmitOptions, EmitOutput,
    EmitWrite, EmitWriteData, EmittedFile, FileCache, Program, ProgramFile, ProgramOptions,
    Session, SessionServices, SourceMapEmitResult,
};
#[cfg(feature = "checker")]
pub use transpile::{
    transpile_declaration, transpile_module, TranspileError, TranspileOptions, TranspileOutput,
    TranspileRequest,
};
/// Bundled-URL host policy, including an explicit immutable library snapshot.
#[cfg(feature = "checker")]
pub use tsr_bundled::BundledFs;
/// Failures from scoped queries, including reentry and retired ownership.
#[cfg(feature = "checker")]
pub use tsr_checker::Error as QueryError;
/// Checker values and formatting flags used by scoped public queries.
#[cfg(feature = "checker")]
pub use tsr_checker::{symbol_format_flags, type_flags, type_format_flags, TypeRef};
/// The request used by compiler-pool diagnostic and emit operations.
#[cfg(feature = "checker")]
pub use tsr_checker::{CheckerRequest as SessionRequest, Operation as QueryOperation};
/// Configuration parsing host using the production package resolver over an
/// explicit filesystem. Use it with `tsr_tsoptions` before session loading.
#[cfg(feature = "checker")]
pub use tsr_compiler::CompilerConfigHost;
/// Failures from loading or emitting a session, including host, ownership
/// and query errors.
#[cfg(feature = "checker")]
pub use tsr_compiler::Error as SessionError;
/// Build-info values and the construction-time reader callback.
#[cfg(feature = "checker")]
pub use tsr_incremental::{BuildInfo, BuildInfoReader};
/// Typed resolution trace arguments returned by the loaded program.
#[cfg(feature = "checker")]
pub use tsr_module::TraceArg;
/// Node-builder formatting flags for scoped declaration queries.
#[cfg(feature = "checker")]
pub use tsr_nodebuilder::{
    flags as node_builder_flags, internal_flags as node_builder_internal_flags,
};

use tsr_ast::{AstFile, SourceFileParseOptions, SourceHash};
use tsr_core::ScriptKind;
use tsr_jsstring::SourceText;

/// Parse already decoded source bytes. No host or checker dependency is needed.
pub fn parse(source: SourceText, kind: ScriptKind, options: SourceFileParseOptions) -> AstFile {
    tsr_parser::parse_source_file(source, kind, options).publish_unbound()
}

/// Parse a fresh source and encode the same content hash and protocol-8 payload
/// as a source loaded by the pinned API server. Hashing is part of this call.
pub fn parse_and_encode(
    source: SourceText,
    kind: ScriptKind,
    options: SourceFileParseOptions,
) -> Result<Vec<u8>, tsr_arena::Error> {
    let hash = xxhash_rust::xxh3::xxh3_128(source.as_bytes());
    let mut parsed = tsr_parser::parse_source_file(source, kind, options);
    parsed.set_source_hash(SourceHash {
        hi: (hash >> 64) as u64,
        lo: hash as u64,
    })?;
    let file = parsed.try_publish_unbound()?;
    tsr_encoder::encode_source_file(
        file.view(),
        file.root().expect("parsed source"),
        &mut tsr_parser::ParserJsDocProvider::default(),
    )
    .map(|encoded| encoded.bytes)
}
