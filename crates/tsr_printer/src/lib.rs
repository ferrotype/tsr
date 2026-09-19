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

mod change_tracker_writer;
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

pub use change_tracker_writer::{
    create_synthetic_source_file, print_and_position_node, ChangeTrackerWriter,
};
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
    /// A child holds another kind of node than the field is typed for, where
    /// upstream's unchecked conversion panics.
    InterfaceConversion {
        found: ts_ast::NodeKind,
        expected: &'static str,
    },
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
            // Upstream's panic text: the context names the switch that had no
            // case for the kind.
            Self::UnexpectedKind { context, kind } => write!(output, "{context}: {kind:?}"),
            Self::MissingNode(what) => write!(output, "missing {what}"),
            Self::InterfaceConversion { found, expected } => write!(
                output,
                "interface conversion: ast.nodeData is *ast.{}, not *ast.{expected}",
                node_data_name(*found)
            ),
        }
    }
}

impl std::error::Error for Error {}

/// The name of upstream's payload struct for a kind, as its panic prints it:
/// the kind's name, with `Node` appended for type nodes, and one shared struct
/// for the keyword types.
fn node_data_name(kind: ts_ast::NodeKind) -> String {
    use ts_ast::SyntaxKind as K;
    let name = format!("{kind:?}");
    let name = name.strip_prefix("Kind").unwrap_or(&name).to_owned();
    match kind.known() {
        Some(kind)
            if (K::FirstTypeNode as u16..=K::LastTypeNode as u16).contains(&(kind as u16)) =>
        {
            format!("{name}Node")
        }
        Some(kind) if (K::FirstKeyword as u16..=K::LastKeyword as u16).contains(&(kind as u16)) => {
            "KeywordTypeNode".to_owned()
        }
        _ => name,
    }
}

#[cfg(test)]
mod printer_tests;
#[cfg(test)]
mod tests;
