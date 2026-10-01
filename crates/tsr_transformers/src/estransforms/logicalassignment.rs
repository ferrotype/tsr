//! `transformers/estransforms/logicalassignment.go`. Not ported yet.
use crate::transformer::{unported, TransformOptions, Transformer};

/// `newLogicalAssignmentTransformer`. Until the port lands, a transformer that fails the file
/// by name, so a chain that needs it is categorized and never passes.
pub fn new_logical_assignment_transformer<'a>(
    opts: &TransformOptions<'a>,
) -> Option<Transformer<'a>> {
    Some(unported(
        opts,
        "estransforms.newLogicalAssignmentTransformer",
    ))
}
