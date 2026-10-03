//! TypeScript's native compiler in Rust: one dependency for the `tsr_*`
//! library crates.
//!
//! tsr is a port of TypeScript 7's Go compiler, held to the pinned upstream
//! test suites. Its libraries are separate crates; this facade re-exports each
//! one as a module named after the package without its `tsr_` prefix, so
//! [`parser`] is `tsr_parser`, [`checker`] is `tsr_checker` and [`core`] is
//! `tsr_core`. It adds no API of its own and stays a plain re-export until the
//! embedding API settles (Phase 7).
//!
//! To parse, check and query programs from Rust, start with [`embed`]
//! (`tsr_embed`): parsing, owned program sessions, diagnostics and scoped type
//! queries. The compiler command line is the separate `tsrust` crate, and the
//! WebAssembly bindings are `tsr_wasm`.

pub use tsr_api as api;
pub use tsr_arena as arena;
pub use tsr_ast as ast;
pub use tsr_astnav as astnav;
pub use tsr_binder as binder;
pub use tsr_bundled as bundled;
pub use tsr_checker as checker;
pub use tsr_compiler as compiler;
pub use tsr_contentmapper as contentmapper;
pub use tsr_core as core;
pub use tsr_diagnostics as diagnostics;
pub use tsr_embed as embed;
pub use tsr_encoder as encoder;
pub use tsr_format as format;
pub use tsr_glob as glob;
pub use tsr_incremental as incremental;
pub use tsr_ipc as ipc;
pub use tsr_jsnum as jsnum;
pub use tsr_json as json;
pub use tsr_jsonrpc as jsonrpc;
pub use tsr_jsstring as jsstring;
pub use tsr_locale as locale;
pub use tsr_module as module;
pub use tsr_nodebuilder as nodebuilder;
pub use tsr_parser as parser;
pub use tsr_printer as printer;
pub use tsr_project as project;
pub use tsr_pseudochecker as pseudochecker;
pub use tsr_scanner as scanner;
pub use tsr_semver as semver;
pub use tsr_sourcemap as sourcemap;
pub use tsr_transformers as transformers;
pub use tsr_transpile as transpile;
pub use tsr_tsoptions as tsoptions;
pub use tsr_tspath as tspath;
pub use tsr_vfs as vfs;
