//! Insertion formatting. The request owns everything it builds: the decoded
//! tree, the positions assigned to it, the synthetic source file over the
//! printed text, the formatting scanner and the edit list. The target file is
//! borrowed and only read. What is returned is independently owned text.

use ts_arena::Counters;
use ts_ast::EagerJsDocProvider;
use ts_encoder::{decode_nodes, DecodeError, DecodedTree};
use ts_format::{FormatCodeSettings, FormatContext, FormatFile};
use ts_printer::EmitContext;

#[derive(Debug, PartialEq, Eq)]
pub enum FormatError {
    Decode(DecodeError),
    Print(ts_printer::Error),
    Format(ts_format::Error),
    /// The formatter's edits overlap or reach outside the printed text.
    Edits(ts_core::UnappliableEdits),
    Arena(ts_arena::Error),
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decode(error) => write!(f, "failed to decode AST: {error}"),
            Self::Print(error) => error.fmt(f),
            Self::Format(error) => error.fmt(f),
            Self::Edits(error) => error.fmt(f),
            Self::Arena(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for FormatError {}

impl From<ts_arena::Error> for FormatError {
    fn from(error: ts_arena::Error) -> Self {
        Self::Arena(error)
    }
}

impl From<ts_format::Error> for FormatError {
    fn from(error: ts_format::Error) -> Self {
        Self::Format(error)
    }
}

/// Decodes protocol-8 syntax and returns it printed and formatted for
/// insertion at a UTF-16 position of the target file.
///
/// Like printing, this needs no checker lease. It reads the target file of a
/// snapshot and must run outside the generation gate.
// source: tsc/internal/api/session.go:Session.handleFormatNodeForInsertion (transport excluded)
pub fn format_node_for_insertion(
    encoded: &[u8],
    target: &mut FormatFile<'_, '_>,
    position: i64,
    settings: &FormatCodeSettings,
    counters: &Counters,
) -> Result<Vec<u8>, FormatError> {
    let scratch = decode_nodes(encoded, counters).map_err(FormatError::Decode)?;
    let position = target
        .view
        .source_file(target.source)?
        .position_map()
        .utf16_to_utf8(position as isize) as i64;
    format_decoded_for_insertion(scratch, target, position, settings)
}

/// The handler's body after decoding, at a byte position of the target file.
/// It consumes the decoded tree: positions are assigned to it in place, and a
/// tree that has been given positions is not printed a second time.
pub fn format_decoded_for_insertion(
    scratch: DecodedTree,
    target: &mut FormatFile<'_, '_>,
    position: i64,
    settings: &FormatCodeSettings,
) -> Result<Vec<u8>, FormatError> {
    // DecodeNodes can return nil for a list-sentinel root, which the pinned
    // printer dereferences. The panic boundary is kept, as printing keeps it.
    let root = scratch
        .root
        .expect("nil root passed to API FormatNodeForInsertion");
    let mut builder = scratch.builder;
    let new_line = settings.editor.new_line_character.clone();
    let indent_size = settings.editor.indent_size;
    let emit_context = EmitContext::new();
    let text = ts_printer::print_and_position_node(
        &mut builder,
        root,
        &new_line,
        indent_size as isize,
        &emit_context,
    )
    .map_err(FormatError::Print)?;

    let (parse_options, language_variant) = {
        let state = target.view.source_file(target.source)?;
        (state.parse_options().clone(), state.language_variant)
    };
    let synthetic =
        ts_printer::create_synthetic_source_file(&mut builder, root, &text, parse_options)
            .map_err(FormatError::Print)?;

    let is_at_line_start =
        ts_format::get_line_start_position_for_position(position, target)? == position;
    let initial_indentation =
        ts_format::get_indentation(target, position, settings, is_at_line_start)?;

    let file = builder.complete(synthetic)?.try_publish_unbound()?;
    let mut jsdoc = EagerJsDocProvider::default();
    let mut synthetic_file = FormatFile {
        view: file.view(),
        source: synthetic,
        jsdoc: &mut jsdoc,
    };
    let delta = if indent_size != 0
        && ts_format::should_indent_child_node(&synthetic_file, settings, root, None, false, false)?
    {
        indent_size
    } else {
        0
    };
    let context = FormatContext::new(settings.clone(), &new_line);
    let changes = ts_format::format_node_given_indentation(
        &mut synthetic_file,
        &context,
        root,
        language_variant,
        initial_indentation,
        delta,
    )?;
    ts_core::apply_bulk_edits(&text, &changes).map_err(FormatError::Edits)
}

#[cfg(test)]
mod scratch_checks;
