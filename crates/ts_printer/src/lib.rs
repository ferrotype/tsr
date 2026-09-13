//! Checker-independent printing (`tsc/internal/printer`).
//!
//! The crate serves the checker's type display first: `TypeToStringEx` builds a
//! type node through the node builder and prints it here; `SymbolToStringEx`
//! prints entity names through the single-line writer with trailing semicolons
//! omitted. JavaScript and declaration emit are Phase 3 and grow the same
//! printer. The dependency direction is fixed: the checker depends on this
//! crate, never the reverse.
//!
//! What is ported: the two text writers, the semicolon-deferring writer, emit
//! flags and list formats, literal text, type-node precedence, and the printer's
//! emission of every type node, type member, parameter, type parameter, entity
//! name and the expressions literal types can hold. Comments, source maps,
//! source-newline preservation, auto-generated names and the statement and
//! expression emitters beyond that set are named boundaries: the printer
//! returns [`Error::Unsupported`] instead of guessing.
//!
//! Output is bytes. Upstream strings may hold arbitrary bytes and the text
//! contract (`docs/design/text.md`) forbids lossy conversion, so writers accept
//! and return `[u8]` and count columns in UTF-16 units the way upstream does.

mod emit_context;
pub mod emit_flags;
pub mod emit_resolver;
mod emit_text_writer;
pub mod list_format;
mod literal_text;
mod printer;
mod semicolon_writer;
mod single_line_string_writer;
mod text_writer;
mod type_precedence;

pub use emit_context::{
    generated_identifier_flags, AutoGenerateId, AutoGenerateInfo, AutoGenerateOptions, EmitContext,
    SynthesizedComment,
};
pub use emit_flags::EmitFlags;
pub use emit_text_writer::EmitTextWriter;
pub use list_format::ListFormat;
pub use literal_text::LiteralTextFlags;
pub(crate) use printer::Session;
pub use printer::{Printer, PrinterOptions, WriteKind};
pub use semicolon_writer::TrailingSemicolonDeferringWriter;
pub use single_line_string_writer::SingleLineStringWriter;
pub use text_writer::{get_default_indent_size, TextWriter};
pub use type_precedence::{get_type_node_precedence, TypePrecedence};

/// Printing failures. Storage failures pass through; the rest name what the
/// printer refused to guess about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Arena(ts_arena::Error),
    /// A configuration or node the port does not print yet, by upstream name.
    Unsupported(&'static str),
    /// A node kind upstream would panic on in this position.
    UnexpectedKind {
        context: &'static str,
        kind: ts_ast::NodeKind,
    },
    /// A required child or payload was absent.
    MissingNode(&'static str),
}

impl From<ts_arena::Error> for Error {
    fn from(error: ts_arena::Error) -> Self {
        Self::Arena(error)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Arena(error) => error.fmt(output),
            Self::Unsupported(name) => write!(output, "printer does not support {name} yet"),
            Self::UnexpectedKind { context, kind } => {
                write!(output, "unexpected node kind {kind:?} in {context}")
            }
            Self::MissingNode(what) => write!(output, "missing {what}"),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod printer_tests;
#[cfg(test)]
mod tests;
