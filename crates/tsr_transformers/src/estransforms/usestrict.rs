//! `transformers/estransforms/usestrict.go`: the `"use strict"` prologue of
//! a file that is not emitted as an ECMAScript module.
use crate::transformer::{EmitModuleFormatOfFile, Failure, TransformOptions, Transformer};
use std::rc::Rc;
use std::sync::Arc;
use tsr_ast::{NodeId, NodeVisitor, SyntaxKind as K};
use tsr_core::{CompilerOptions, ModuleKind, ScriptKind};
use tsr_printer::EmitContext;

// port: tsc/internal/transformers/estransforms/usestrict.go:NewUseStrictTransformer
pub fn new_use_strict_transformer<'a>(opts: &TransformOptions<'a>) -> Option<Transformer<'a>> {
    let tx = Rc::new(UseStrictTransformer {
        emit_context: opts.context.clone(),
        compiler_options: Arc::clone(&opts.compiler_options),
        get_emit_module_format_of_file: Rc::clone(&opts.get_emit_module_format_of_file),
        failure: opts.failure.clone(),
    });
    Some(Transformer::new(
        move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| tx.visit(visitor, node),
        Some(opts.context.clone()),
        opts.failure.clone(),
    ))
}

struct UseStrictTransformer<'a> {
    emit_context: EmitContext,
    compiler_options: Arc<CompilerOptions>,
    get_emit_module_format_of_file: EmitModuleFormatOfFile<'a>,
    failure: Failure,
}

impl UseStrictTransformer<'_> {
    // port: tsc/internal/transformers/estransforms/usestrict.go:useStrictTransformer.visit
    fn visit(&self, visitor: &mut NodeVisitor<'_>, node: Option<NodeId>) -> Option<NodeId> {
        if self.failure.is_set() {
            return node;
        }
        let id = node.expect("runtime error: invalid memory address or nil pointer dereference");
        if visitor.factory().node(id).kind() != K::SourceFile {
            return node;
        }
        Some(self.visit_source_file(visitor, id))
    }

    // port: tsc/internal/transformers/estransforms/usestrict.go:useStrictTransformer.visitSourceFile
    fn visit_source_file(&self, visitor: &mut NodeVisitor<'_>, node: NodeId) -> NodeId {
        let file = match visitor.factory().read_source_file(node) {
            Ok(file) => file,
            Err(error) => {
                self.failure.record(error);
                return node;
            }
        };
        if file.script_kind == ScriptKind::JSON {
            return node;
        }

        let is_external_module = tsr_ast::utilities::is_external_module(&file);
        let file_name = file.file_name();
        let module_kind = self.compiler_options.emit_module_kind();
        let format = match (self.get_emit_module_format_of_file)(file_name) {
            Ok(format) => format,
            Err(error) => {
                self.failure.record(error);
                return node;
            }
        };

        // ESM is always strict. If the file is ESM, and CJS emit
        // has not been requested, then skip adding "use strict".
        if is_external_module
            && module_kind >= ModuleKind::ES2015
            && (module_kind == ModuleKind::PRESERVE || format >= ModuleKind::ES2015)
        {
            return node;
        }

        let factory = visitor.factory_mut();
        let (statements, end_of_file_token) = {
            let read = factory.node(node);
            let data = read.as_source_file().expect("SourceFile payload");
            (data.statements(), data.end_of_file_token())
        };
        let statements =
            statements.expect("runtime error: invalid memory address or nil pointer dereference");
        let nodes = factory.read_list(statements).nodes();
        let nodes: Vec<NodeId> = factory
            .read_nodes(nodes)
            .iter()
            .map(|statement| statement.expect("nil node in statement list"))
            .collect();
        let statements_loc = factory.read_list(statements).loc();
        let nodes = self.emit_context.ensure_use_strict(factory, nodes);
        let slice = factory.alloc_nodes(nodes.into_iter().map(Some).collect());
        let statement_list = factory.alloc_list(tsr_core::TextRange::new(-1, -1), slice);
        factory.set_list_location(statement_list, statements_loc);
        factory.update_source(node, Some(statement_list), end_of_file_token)
    }
}
