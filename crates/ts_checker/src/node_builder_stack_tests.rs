//! Native stack growth and unwind through the real checker operation.
use super::NodeBuilder;
use crate::{CheckerOptions, CheckerOwner, Error};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use ts_arena::{CheckerIdentity, Counters, Generation};
use ts_ast::{AstBuilder, Factory, FactoryHooks, NodeId};
use ts_jsstring::SourceText;
use ts_nodebuilder::flags as nf;
use ts_printer::{EmitTextWriter, Printer, PrinterOptions, TextWriter};

struct ObservedHooks {
    inner: Arc<dyn FactoryHooks>,
    largest_stack: Arc<AtomicUsize>,
    panic_above_stack: Option<usize>,
}
impl FactoryHooks for ObservedHooks {
    fn on_create(&self, factory: &mut dyn Factory, node: NodeId) {
        let remaining = stacker::remaining_stack().expect("native stack bounds");
        self.largest_stack.fetch_max(remaining, Ordering::Relaxed);
        assert!(
            self.panic_above_stack
                .is_none_or(|limit| remaining <= limit),
            "factory panic inside grown builder stack"
        );
        self.inner.on_create(factory, node);
    }
    fn on_update(&self, factory: &mut dyn Factory, node: NodeId, original: NodeId) {
        self.inner.on_update(factory, node, original);
    }
    fn on_clone(&self, factory: &mut dyn Factory, node: NodeId, original: NodeId) {
        self.inner.on_clone(factory, node, original);
    }
}

fn observe(
    builder: &mut NodeBuilder<'_>,
    largest_stack: &Arc<AtomicUsize>,
    panic_above_stack: Option<usize>,
) {
    builder.ast = AstBuilder::with_hooks(
        SourceText::from_bytes(b"".as_slice()),
        &builder.checker.counters,
        Arc::new(ObservedHooks {
            inner: builder.emit.factory_hooks(),
            largest_stack: Arc::clone(largest_stack),
            panic_above_stack,
        }),
    );
}

#[test]
fn deep_type_display_grows_and_factory_panic_retires_the_operation() {
    const STACK: usize = 512 * 1024;
    const DEPTH: usize = 3000;
    std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(|| {
            let counters = Counters::new();
            let before = counters.snapshot();
            let identity = CheckerIdentity::new(Generation::new(&counters), &counters);
            let owner = Arc::new(
                CheckerOwner::new(identity, &counters, CheckerOptions::default()).unwrap(),
            );
            let largest_stack = Arc::new(AtomicUsize::new(0));
            let ty;
            {
                let mut operation = owner.operation().unwrap();
                let checker = operation.state_mut();
                let mut root = checker.builtins.string_type;
                for _ in 0..DEPTH {
                    root = checker.new_index_type(root, 0).unwrap();
                }
                ty = root;
                let expected = format!("{}string", "keyof ".repeat(DEPTH));
                let chain = (0..DEPTH)
                    .map(|_| {
                        checker
                            .new_symbol(
                                ts_ast::symbol_flags::TYPE_ALIAS,
                                ts_ast::JsString::from_bytes(b"n".as_slice()),
                            )
                            .unwrap()
                    })
                    .collect::<Vec<_>>();
                let storage_before = counters.snapshot();
                {
                    let mut builder = NodeBuilder::new(checker, nf::NO_TRUNCATION);
                    observe(&mut builder, &largest_stack, None);
                    let node = builder.type_node(ty).unwrap();
                    assert!(
                        largest_stack.load(Ordering::Relaxed) > STACK,
                        "type builder must visit a grown segment"
                    );
                    let mut writer = TextWriter::new(b"", 0);
                    Printer::new(PrinterOptions::default(), &builder.emit)
                        .write(builder.ast.view(), node, None, &mut writer)
                        .unwrap();
                    assert_eq!(writer.text(), expected.as_bytes());
                }
                // Exercise the production byte-returning path as well as the explicit builder.
                assert_eq!(
                    checker
                        .type_to_string(ty, crate::type_format_flags::NO_TRUNCATION)
                        .unwrap()
                        .as_bytes(),
                    expected.as_bytes()
                );
                largest_stack.store(0, Ordering::Relaxed);
                {
                    let mut builder = NodeBuilder::new(
                        checker,
                        nf::NO_TRUNCATION | nf::FORBID_INDEXED_ACCESS_SYMBOL_REFERENCES,
                    );
                    observe(&mut builder, &largest_stack, None);
                    let name = builder
                        .access_from_symbol_chain(&chain, chain.len() - 1, 0, None)
                        .unwrap();
                    assert!(
                        largest_stack.load(Ordering::Relaxed) > STACK,
                        "symbol access construction must grow the stack"
                    );
                    let mut writer = TextWriter::new(b"", 0);
                    Printer::new(PrinterOptions::default(), &builder.emit)
                        .write(builder.ast.view(), name, None, &mut writer)
                        .unwrap();
                    assert_eq!(writer.text(), vec!["n"; DEPTH].join(".").as_bytes());
                }
                assert_eq!(
                    counters.snapshot(),
                    storage_before,
                    "successful display releases active AST storage"
                );
            }
            largest_stack.store(0, Ordering::Relaxed);
            let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut operation = owner.operation().unwrap();
                NodeBuilder::with_cached(operation.state_mut(), nf::NO_TRUNCATION, |builder| {
                    // Release frame sizes can put the first leaf on the original
                    // stack. Trigger only when a hook observes a grown segment.
                    observe(builder, &largest_stack, Some(STACK));
                    builder.type_node(ty)?;
                    Ok(ts_ast::JsString::default())
                })
                .unwrap();
            }));
            assert!(panic.is_err());
            assert!(
                largest_stack.load(Ordering::Relaxed) > STACK,
                "panic must occur after stack growth"
            );
            assert!(matches!(
                owner.operation(),
                Err(Error::Arena(ts_arena::Error::Retired))
            ));
            drop(owner);
            assert_eq!(
                counters.snapshot(),
                before,
                "unwind releases all builder and checker owners"
            );
        })
        .unwrap()
        .join()
        .unwrap();
}
