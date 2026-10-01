//! `transformers/estransforms/esdecorator.go`. Not ported yet.
use crate::transformer::{unported, TransformOptions, Transformer};

/// `newESDecoratorTransformer`. Until the port lands, a transformer that fails the file
/// by name, so a chain that needs it is categorized and never passes.
pub fn new_es_decorator_transformer<'a>(opts: &TransformOptions<'a>) -> Option<Transformer<'a>> {
    Some(unported(opts, "estransforms.newESDecoratorTransformer"))
}
