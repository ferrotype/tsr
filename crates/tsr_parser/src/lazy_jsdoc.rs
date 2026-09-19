//! The parser service used by AST consumers for first-use JSDoc parsing.
use crate::{jsdoc::get_jsdoc_comment_ranges, on_parser_worker, Parser};
use ts_arena::Error;
use ts_ast::{node_flags, AstView, BorrowedFactory, Factory, JSDocRoots, JsDocProvider, NodeId};

/// Reuse one provider for an encoding/indexing operation. Ordinary nodes return
/// the shared empty result; cache hits do not enter a worker or create a parser.
/// First use enters the parser worker before acquiring the lazy publication lock.
#[derive(Debug)]
pub struct ParserJsDocProvider {
    empty: JSDocRoots,
}

impl Default for ParserJsDocProvider {
    fn default() -> Self {
        Self {
            empty: JSDocRoots::empty(),
        }
    }
}

impl JsDocProvider for ParserJsDocProvider {
    /// port: tsc/internal/ast/ast.go:Node.JSDoc
    fn jsdoc(
        &mut self,
        view: AstView<'_>,
        source: NodeId,
        parent: NodeId,
    ) -> Result<JSDocRoots, Error> {
        if view.node(parent)?.flags() & node_flags::HAS_JS_DOC == 0 {
            return Ok(self.empty.clone());
        }
        if let Some(roots) = view.source_eager_jsdoc(source, parent)? {
            return Ok(roots);
        }
        let source_owner = view.for_node_owner(source)?;
        let metadata = source_owner.source_file(source)?;
        if !metadata.has_lazy_jsdoc {
            return Ok(self.empty.clone());
        }
        // The cache and transaction are scoped to the parent's storage owner.
        // Reject a supplied source from another owner before it could populate
        // that cache using unrelated source bytes.
        if source_owner.file_info().root != view.for_node_owner(parent)?.file_info().root {
            return Err(Error::WrongOwner);
        }
        on_parser_worker(|| {
            source_owner.source_jsdoc(source, parent, |transaction| {
                // port: tsc/internal/parser/jsdoc.go:parseJSDocForNode
                let mut parser = Parser::new(
                    metadata.parse_options().clone(),
                    metadata.text(),
                    metadata.script_kind,
                    BorrowedFactory(transaction),
                );
                let ranges = get_jsdoc_comment_ranges(
                    &parser.factory,
                    Vec::new(),
                    parent,
                    parser.source_text,
                );
                let mut roots = Vec::with_capacity(ranges.len());
                let mut pos = parser.factory.node(parent).range().pos();
                for comment in ranges {
                    if let Some(parsed) = parser.parse_js_doc_comment(
                        parent,
                        comment.loc.pos(),
                        comment.loc.end(),
                        pos,
                    ) {
                        Factory::set_node_parent(&mut parser.factory, parsed, Some(parent));
                        roots.push(parsed);
                        pos = parser.factory.node(parsed).range().end();
                    }
                }
                Ok(roots)
            })
        })
    }
}
