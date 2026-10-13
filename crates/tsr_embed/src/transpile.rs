//! Single-file operations use the production transpile API directly, including
//! its option overrides and synthetic in-memory host. They do not substitute
//! whole-program emit. Reexporting the existing types keeps one options,
//! ownership and cancellation contract for both entry points.

/// A transpilation request. Its optional cancellation token is polled by
/// module emit. Declaration transpilation forces emit and skips that gate,
/// matching the pinned API even for an already canceled token.
pub use tsr_checker::CheckerRequest as TranspileRequest;

/// A program-loading, checking or emission failure. Cancellation of a module
/// transpilation is instead the successful result `Ok(None)`.
pub use tsr_compiler::Error as TranspileError;

/// Borrowed base compiler options, input filename and diagnostic preference.
/// The operation copies the compiler options before overriding the settings
/// required for single-file transpilation.
///
/// ```
/// use tsr_embed::{
///     transpile_declaration, transpile_module, TranspileOptions, TranspileRequest,
/// };
///
/// let request = TranspileRequest::default();
/// let options = TranspileOptions::default();
/// let input = b"export const x: number = 1;\n";
/// let module = transpile_module(&request, input, &options).unwrap().unwrap();
/// assert_eq!(module.output_text.as_bytes(), b"export const x = 1;\n");
/// let declaration = transpile_declaration(&request, input, &options).unwrap().unwrap();
/// assert_eq!(declaration.output_text.as_bytes(), b"export declare const x: number;\n");
/// ```
pub use tsr_transpile::Options as TranspileOptions;

/// Owned text, source-map text and diagnostics. The retained `program` owns
/// the source files identified by the diagnostics, so it must accompany any
/// diagnostics that escape the result. Input and options may be dropped as
/// soon as the call returns; cloning diagnostics alone does not retain files.
pub use tsr_transpile::Output as TranspileOutput;

pub use tsr_transpile::{transpile_declaration, transpile_module};
