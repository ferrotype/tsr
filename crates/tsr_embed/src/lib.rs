//! Rust embedding APIs. Disable default features for the parser-only surface.
//!
//! Loaded source preserves its original bytes. The host supplies canonical
//! source-file options. The checker feature adds an immutable program session
//! with scoped queries and explicit retained handles.

#[cfg(feature = "checker")]
mod session;
#[cfg(feature = "checker")]
pub use session::{FileCache, Program, ProgramOptions, Session};

use ts_ast::{AstFile, SourceFileParseOptions, SourceHash};
use ts_core::ScriptKind;
use ts_jsstring::SourceText;

/// Parse already decoded source bytes. No host or checker dependency is needed.
pub fn parse(source: SourceText, kind: ScriptKind, options: SourceFileParseOptions) -> AstFile {
    ts_parser::parse_source_file(source, kind, options).publish_unbound()
}

/// Parse a fresh source and encode the same content hash and protocol-8 payload
/// as a source loaded by the pinned API server. Hashing is part of this call.
pub fn parse_and_encode(
    source: SourceText,
    kind: ScriptKind,
    options: SourceFileParseOptions,
) -> Result<Vec<u8>, ts_arena::Error> {
    let hash = xxhash_rust::xxh3::xxh3_128(source.as_bytes());
    let mut parsed = ts_parser::parse_source_file(source, kind, options);
    parsed.set_source_hash(SourceHash {
        hi: (hash >> 64) as u64,
        lo: hash as u64,
    })?;
    let file = parsed.try_publish_unbound()?;
    ts_encoder::encode_source_file(
        file.view(),
        file.root().expect("parsed source"),
        &mut ts_parser::ParserJsDocProvider::default(),
    )
    .map(|encoded| encoded.bytes)
}
