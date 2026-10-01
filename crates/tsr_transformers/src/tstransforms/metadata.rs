//! `transformers/tstransforms/metadata.go`. Not ported yet.
use crate::transformer::{unported, TransformOptions, Transformer};

/// `NewMetadataTransformer`. Until the port lands, a transformer that fails the file
/// by name, so a chain that needs it is categorized and never passes.
pub fn new_metadata_transformer<'a>(opts: &TransformOptions<'a>) -> Option<Transformer<'a>> {
    Some(unported(opts, "tstransforms.NewMetadataTransformer"))
}
