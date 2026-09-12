//! The print request owns its decoded syntax until emission finishes. Neither
//! its node ids nor the printer's side tables enter a snapshot or registry.

use ts_arena::Counters;
use ts_encoder::{decode_nodes, DecodeError, DecodedTree};
use ts_printer::{EmitContext, Printer, PrinterOptions};

/// The three options accepted by the pinned API's PrintNode request. Transport
/// base64 decoding is the caller's responsibility; text remains arbitrary bytes.
#[derive(Clone, Copy, Debug, Default)]
pub struct PrintNodeOptions {
    pub preserve_source_newlines: bool,
    pub never_ascii_escape: bool,
    pub terminate_unterminated_literals: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PrintError {
    Decode(DecodeError),
    Print(ts_printer::Error),
}
impl std::fmt::Display for PrintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decode(error) => write!(f, "failed to decode AST: {error}"),
            Self::Print(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for PrintError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Decode(error) => Some(error),
            Self::Print(error) => Some(error),
        }
    }
}

/// Decode protocol-8 syntax and return independently owned printed bytes.
///
/// This operation needs no checker lease or snapshot. It must run outside the
/// generation gate; a snapshot-backed caller stages the resulting bytes through
/// `Snapshot::prepare` and `Snapshot::commit` only after scratch has dropped.
/// The current printer's unsupported syntax/options remain explicit errors.
/// Decoder panics retain the pinned boundary and unwind request scratch; callers
/// that recover a snapshot request use `Snapshot::request` for retirement.
// source: tsc/internal/api/session.go:Session.handlePrintNode (transport excluded)
pub fn print_node(
    encoded: &[u8],
    options: PrintNodeOptions,
    counters: &Counters,
) -> Result<Vec<u8>, PrintError> {
    let scratch = decode_nodes(encoded, counters).map_err(PrintError::Decode)?;
    print_decoded(scratch, options)
}

// Consume the actual compact AST builder as request scratch. It is deliberately
// unpublished: returning text never requires a long-lived AstFile or registry id.
fn print_decoded(scratch: DecodedTree, options: PrintNodeOptions) -> Result<Vec<u8>, PrintError> {
    // DecodeNodes can return nil for a list-sentinel root. Pinned Printer.Emit
    // dereferences it; keep the panic boundary with a diagnostic Rust message.
    let root = scratch.root.expect("nil root passed to API PrintNode");
    let context = EmitContext::new();
    let mut printer = Printer::new(
        PrinterOptions {
            preserve_source_newlines: options.preserve_source_newlines,
            never_ascii_escape: options.never_ascii_escape,
            terminate_unterminated_literals: options.terminate_unterminated_literals,
            ..PrinterOptions::default()
        },
        &context,
    );
    let result = printer
        .emit(scratch.builder.view(), root, None)
        .map_err(PrintError::Print);
    drop(scratch);
    result
}

#[cfg(test)]
mod scratch_checks;
