//! The parser supplies lazy JSDoc; syntax and encoder crates do not depend on it.
use crate::{AstView, JSDocRoots, NodeId};
use ts_arena::Error;

pub trait JsDocProvider {
    fn jsdoc(
        &mut self,
        view: AstView<'_>,
        source: NodeId,
        parent: NodeId,
    ) -> Result<JSDocRoots, Error>;
}

/// Explicit eager-only service for constructed trees and decoder fixtures.
/// An unseeded parent of a file with lazy JSDoc is an error, never an empty result.
#[derive(Debug, Default)]
pub struct EagerJsDocProvider {
    empty: JSDocRoots,
}
impl JsDocProvider for EagerJsDocProvider {
    fn jsdoc(
        &mut self,
        view: AstView<'_>,
        source: NodeId,
        parent: NodeId,
    ) -> Result<JSDocRoots, Error> {
        if view.node(parent)?.flags() & crate::node_flags::HAS_JS_DOC == 0 {
            return Ok(self.empty.clone());
        }
        if let Some(roots) = view.source_eager_jsdoc(source, parent)? {
            return Ok(roots);
        }
        if view.source_file(source)?.has_lazy_jsdoc {
            return Err(Error::InvalidGraph);
        }
        Ok(self.empty.clone())
    }
}
