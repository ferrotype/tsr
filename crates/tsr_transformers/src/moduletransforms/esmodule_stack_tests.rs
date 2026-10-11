//! Native equivalence for the wasm ES module expression continuation. Each
//! path gets a separate parsed owner, builder and emit context.
use super::*;
use std::ops::ControlFlow;
use tsr_arena::Counters;
use tsr_ast::{AstBuilder, AstView, ChildVisitor, NodeSlice, SourceFileParseOptions};
use tsr_core::{NewLineKind, ScriptKind, Tristate};
use tsr_jsstring::SourceText;
use tsr_printer::{Printer, PrinterOptions};

struct Children<'a>(AstView<'a>, Vec<NodeId>);
impl ChildVisitor for Children<'_> {
    fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
        self.1.push(node);
        ControlFlow::Continue(())
    }
    fn visit_list(&mut self, list: NodeListId) -> ControlFlow<()> {
        self.visit_node_slice(self.0.list(list).expect("list").nodes())
    }
    fn visit_node_slice(&mut self, nodes: NodeSlice) -> ControlFlow<()> {
        self.1
            .extend(self.0.node_slice(nodes).expect("nodes").iter().flatten());
        ControlFlow::Continue(())
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Observation {
    output: Vec<u8>,
    nodes: Vec<String>,
    visits: Vec<String>,
    failure: Option<String>,
}

fn observe(text: &str, iterative: bool, fail_at_stop: bool) -> Observation {
    observe_after_type_erasure(text, iterative, fail_at_stop, false)
}

fn observe_after_type_erasure(
    text: &str,
    iterative: bool,
    fail_at_stop: bool,
    erase: bool,
) -> Observation {
    let parsed = tsr_parser::parse_source_file(
        SourceText::from_loaded_bytes(text.as_bytes()),
        ScriptKind::TS,
        SourceFileParseOptions {
            file_name: JsString::from_bytes(&b"/main.ts"[..]),
            path: JsString::from_bytes(&b"/main.ts"[..]),
            ..Default::default()
        },
    );
    assert!(parsed
        .view()
        .source_file(parsed.root())
        .expect("source")
        .diagnostics
        .is_empty());
    let context = EmitContext::new();
    let mut factory = AstBuilder::with_hooks(
        SourceText::default(),
        &Counters::new(),
        context.factory_hooks(),
    );
    let mut root = parsed.root();
    factory.retain_file(parsed.publish_unbound());
    let mut options = crate::transformer_tests::options(&context);
    options.compiler_options = Arc::new(CompilerOptions {
        module: ModuleKind::ESNEXT,
        rewrite_relative_import_extensions: Tristate::TRUE,
        ..Default::default()
    });
    options.get_emit_module_format_of_file = Rc::new(|_| Ok(ModuleKind::ESNEXT));
    if erase {
        root = crate::tstransforms::typeeraser::new_type_eraser_transformer(&options)
            .expect("type eraser")
            .transform_source_file(&mut factory, root)
            .expect("type erasure");
    }
    let tx = ESModuleTransformer {
        context: context.clone(),
        compiler_options: Arc::clone(&options.compiler_options),
        get_emit_module_format_of_file: Rc::clone(&options.get_emit_module_format_of_file),
        failure: options.failure.clone(),
        current_source_file: Cell::new(None),
        import_require_statements: RefCell::new(None),
        iterative_spines: iterative,
    };
    let visits = RefCell::new(Vec::new());
    let transformer = Transformer::new(
        |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| {
            if let Some(id) = node {
                let read = visitor.factory().node(id);
                // Spine callbacks become frames. Every other callback must
                // still occur in the same order, including environment hooks.
                if !matches!(
                    read.kind().known(),
                    Some(
                        K::BinaryExpression
                            | K::ParenthesizedExpression
                            | K::PartiallyEmittedExpression
                    )
                ) {
                    visits
                        .borrow_mut()
                        .push(format!("{:?}@{:?}", read.kind(), read.range()));
                }
                if fail_at_stop
                    && read
                        .as_identifier()
                        .is_some_and(|identifier| identifier.text() == b"stop")
                {
                    options
                        .failure
                        .record(Error::Unsupported("test visit failure"));
                }
            }
            tx.visit(visitor, node)
        },
        Some(context.clone()),
        options.failure.clone(),
    );
    let result = transformer.visitor(&mut factory).visit_source_file(root);
    let failure = options.failure.take().map(|error| error.to_string());
    drop(transformer);
    let view = factory.view();
    let mut pending = vec![result];
    let mut nodes = Vec::new();
    while let Some(id) = pending.pop() {
        let read = view.node(id).expect("node");
        let original = context.original(id).map(|node| {
            let original = view.node(node).expect("original node");
            format!("{:?}@{:?}", original.kind(), original.range())
        });
        nodes.push(format!(
            "{:?}@{:?}/{:?}/{original:?}",
            read.kind(),
            read.range(),
            read.flags()
        ));
        let mut children = Children(view, Vec::new());
        let _ = read.for_each_child(&mut children);
        pending.extend(children.1.into_iter().rev());
    }
    let output = Printer::new(
        PrinterOptions {
            new_line: NewLineKind::LF,
            ..Default::default()
        },
        &context,
    )
    .emit_source_file(view, result)
    .expect("print transformed file");
    Observation {
        output,
        nodes,
        visits: visits.into_inner(),
        failure,
    }
}

#[test]
fn wasm_esmodule_spines_match_recursive_updates_and_visit_order() {
    let cases = [
        "export const x = a + b * c - d / e;",
        "export const x = a + (b + (c + d));",
        "export const x = (a = b = c), y = a && b || c ?? d;",
        "export const x = import('./a.ts') + (import('./b.mts') + import('./c.cts'));",
        "export const x = (import('./a.ts') + import('./b.ts')) + import('./c.ts');",
        "export const x = import(path) + import('./b.ts');",
        "export const x = (() => { const y = import('./a.ts') + import('./b.ts'); return y; }) + (function (p = import('./c.ts') + import('./d.ts')) { return p; });",
        "export const x = (/*left*/ import('./a.ts') /*middle*/ + import('./b.ts') /*right*/);\n(x + y)\n[0];",
        "export const x = ({ a } = object) + (a += 1);",
    ];
    for text in cases {
        let recursive = observe(text, false, false);
        let iterative = observe(text, true, false);
        assert_eq!(iterative, recursive, "{text}");
        assert!(iterative.failure.is_none());
    }
    let rewritten = observe(cases[3], true, false);
    assert!(rewritten
        .output
        .windows(b"./a.js".len())
        .any(|part| part == b"./a.js"));
    assert!(!rewritten
        .output
        .windows(b"./a.ts".len())
        .any(|part| part == b"./a.ts"));
}

#[test]
fn wasm_esmodule_spines_unwind_after_first_failure() {
    for text in [
        "export const x = (import('./a.ts') + stop) + (import('./b.ts') + stop);",
        "export const x = (import('./a.ts') + stop) + import('./b.ts');",
    ] {
        let recursive = observe(text, false, true);
        let iterative = observe(text, true, true);
        assert_eq!(iterative, recursive, "{text}");
        assert_eq!(
            iterative.failure.as_deref(),
            Some("unsupported: test visit failure")
        );
    }
}

#[test]
fn wasm_esmodule_spines_handle_the_required_deep_binary_fixture() {
    let chain = std::iter::repeat_n("a", 5_001)
        .collect::<Vec<_>>()
        .join(" + ");
    let text = format!("export const x = {chain};\n");
    let recursive = observe(&text, false, false);
    let iterative = observe(&text, true, false);
    assert_eq!(iterative, recursive);
    assert_eq!(iterative.output, text.as_bytes());
}

#[test]
fn wasm_esmodule_spines_preserve_partially_emitted_updates() {
    let text = "export const x = ((import('./a.ts') as any) + (import('./b.ts') as any));";
    let recursive = observe_after_type_erasure(text, false, false, true);
    let iterative = observe_after_type_erasure(text, true, false, true);
    assert_eq!(iterative, recursive);
    assert!(iterative
        .nodes
        .iter()
        .any(|node| node.contains("PartiallyEmittedExpression")));
    assert!(iterative
        .output
        .windows(b"./a.js".len())
        .any(|part| part == b"./a.js"));
}
