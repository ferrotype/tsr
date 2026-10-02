//! `transformers/moduletransforms/impliedmodule.go`: transforms each file with
//! the CommonJS or the ES module transformer, by the file's emit module format.
use crate::moduletransforms::{commonjsmodule, esmodule};
use crate::transformer::{TransformOptions, Transformer};
use std::cell::RefCell;
use std::rc::Rc;
use tsr_ast::{NodeId, NodeVisitor, SyntaxKind as K};
use tsr_core::ModuleKind;

const NIL: &str = "runtime error: invalid memory address or nil pointer dereference";

/// `ImpliedModuleTransformer`. The component transformers are created on
/// first use and kept; each is cloned out of its cell before it runs.
struct ImpliedModuleTransformer<'a> {
    opts: TransformOptions<'a>,
    cjs_transformer: RefCell<Option<Rc<Transformer<'a>>>>,
    esm_transformer: RefCell<Option<Rc<Transformer<'a>>>>,
}

// port: tsc/internal/transformers/moduletransforms/impliedmodule.go:NewImpliedModuleTransformer
pub fn new_implied_module_transformer<'a>(opts: &TransformOptions<'a>) -> Option<Transformer<'a>> {
    let tx = Rc::new(ImpliedModuleTransformer {
        opts: opts.clone(),
        cjs_transformer: RefCell::new(None),
        esm_transformer: RefCell::new(None),
    });
    Some(Transformer::new(
        move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| tx.visit(visitor, node),
        Some(opts.context.clone()),
        opts.failure.clone(),
    ))
}

impl<'a> ImpliedModuleTransformer<'a> {
    // port: tsc/internal/transformers/moduletransforms/impliedmodule.go:ImpliedModuleTransformer.visit
    fn visit(&self, visitor: &mut NodeVisitor<'_>, node: Option<NodeId>) -> Option<NodeId> {
        if self.opts.failure.is_set() {
            return node;
        }
        let mut node = node.expect(NIL);
        if visitor.factory().node(node).kind() == K::SourceFile {
            node = self.visit_source_file(visitor, node);
        }
        Some(node)
    }

    // port: tsc/internal/transformers/moduletransforms/impliedmodule.go:ImpliedModuleTransformer.visitSourceFile
    fn visit_source_file(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let failure = &self.opts.failure;
        let file = visitor.factory().read_source_file(node);
        let Some(file) = failure.ok(file) else {
            return node;
        };
        if file.is_declaration_file {
            return node;
        }
        let file_name = file.file_name().to_vec();
        drop(file);

        let format = (self.opts.get_emit_module_format_of_file)(&file_name);
        let Some(format) = failure.ok(format) else {
            return node;
        };

        let transformer = if format >= ModuleKind::ES2015 {
            Self::component(&self.esm_transformer, || {
                esmodule::new_es_module_transformer(&self.opts)
            })
        } else {
            Self::component(&self.cjs_transformer, || {
                commonjsmodule::new_common_js_module_transformer(&self.opts)
            })
        };
        let transformer = transformer.expect(NIL);

        match transformer.transform_source_file(visitor.factory_mut(), node) {
            Ok(transformed) => transformed,
            Err(error) => {
                failure.record(error);
                node
            }
        }
    }

    /// `if tx.x == nil { tx.x = New...(tx.opts) }; transformer = tx.x`.
    fn component(
        slot: &RefCell<Option<Rc<Transformer<'a>>>>,
        create: impl FnOnce() -> Option<Transformer<'a>>,
    ) -> Option<Rc<Transformer<'a>>> {
        if slot.borrow().is_none() {
            let created = create().map(Rc::new);
            *slot.borrow_mut() = created;
        }
        slot.borrow().clone()
    }
}
