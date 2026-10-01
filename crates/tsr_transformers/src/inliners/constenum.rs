//! `transformers/inliners/constenum.go`. Not ported yet.
use crate::transformer::{unported, TransformOptions, Transformer};

/// `NewConstEnumInliningTransformer`. Until the port lands, a transformer that fails the file
/// by name, so a chain that needs it is categorized and never passes.
pub fn new_const_enum_inlining_transformer<'a>(
    opts: &TransformOptions<'a>,
) -> Option<Transformer<'a>> {
    Some(unported(opts, "inliners.NewConstEnumInliningTransformer"))
}
