//! `transformers/tstransforms/runtimesyntax.go`. Not ported yet.
use crate::transformer::{unported, TransformOptions, Transformer};

/// `NewRuntimeSyntaxTransformer`. Until the port lands, a transformer that fails the file
/// by name, so a chain that needs it is categorized and never passes.
pub fn new_runtime_syntax_transformer<'a>(opts: &TransformOptions<'a>) -> Option<Transformer<'a>> {
    Some(unported(opts, "tstransforms.NewRuntimeSyntaxTransformer"))
}
