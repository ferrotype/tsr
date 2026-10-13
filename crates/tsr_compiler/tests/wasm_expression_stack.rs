//! Native equivalence witnesses for the exact expression driver used by wasm.
//! Fresh checker owners prevent an earlier route from warming the other's caches.
#![cfg(feature = "recursion-probe")]

use std::sync::Arc;
use tsr_arena::{CheckerIdentity, Counters, Generation, NodeId};
use tsr_ast::{Diagnostic, SyntaxKind};
use tsr_checker::{CheckerOwner, Error, MemoryTraceSink, TraceEvent, Tracer};
use tsr_compiler::{FileCache, Program, ProgramCheckerHost, ProgramOptions};
use tsr_core::{CompilerOptions, ModuleKind, ScriptTarget, Tristate};
use tsr_jsstring::JsString;

fn program(text: &[u8]) -> (Arc<Program>, Counters) {
    let counters = Counters::new();
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    fs.insert_loaded(b"/main.ts", text);
    let program = Program::load(
        ProgramOptions {
            config: tsr_tsoptions::ParsedCommandLine::new(
                CompilerOptions {
                    target: ScriptTarget::ESNEXT,
                    module: ModuleKind::ESNEXT,
                    strict: Tristate::TRUE,
                    no_lib: Tristate::TRUE,
                    ..Default::default()
                },
                vec![JsString::from_bytes(b"/main.ts".as_slice())],
            ),
            host: Arc::new(fs.finish()),
            current_directory: JsString::from_bytes(b"/".as_slice()),
            default_library_path: JsString::from_bytes(b"/no-default-lib".as_slice()),
            skip_module_resolution: false,
            single_threaded: Tristate::TRUE,
        },
        &mut FileCache::new(),
        &counters,
    )
    .unwrap();
    (Arc::new(program), counters)
}

fn initializer(program: &Program, name: &[u8]) -> NodeId {
    let file = program.source_file(b"/main.ts").unwrap();
    let view = file.bound().view().ast();
    let mut pending = vec![file.source()];
    while let Some(node) = pending.pop() {
        let read = view.node(node).unwrap();
        if read.kind() == SyntaxKind::VariableDeclaration {
            if let Some(identifier) = read.name() {
                if view.node_text(identifier).unwrap().as_bytes() == name {
                    return read.initializer().unwrap();
                }
            }
        }
        for visit in tsr_astnav::visit_each_child(view, node)
            .unwrap()
            .into_iter()
            .rev()
        {
            match visit {
                tsr_astnav::Visit::Node(node) => pending.push(node),
                tsr_astnav::Visit::List(list) => pending.extend(
                    view.node_slice(view.list(list).unwrap().nodes())
                        .unwrap()
                        .iter()
                        .flatten()
                        .rev(),
                ),
            }
        }
    }
    panic!("missing query variable");
}

#[derive(Debug, PartialEq, Eq)]
struct Observation {
    // Full records compare every byte string, chain, related diagnostic and flag,
    // in returned order. Both owners use the same immutable source identities.
    diagnostics: Vec<Diagnostic>,
    suggestions: Vec<Diagnostic>,
    queried_type: Vec<u8>,
    types: usize,
    symbols: u32,
    signatures: usize,
    instantiations: u64,
    expressions: Vec<TraceEvent>,
    maximum_expression_depth: u64,
}

fn observe(program: &Arc<Program>, counters: &Counters, iterative: bool) -> Observation {
    // false retains sampled checkExpression events; true would suppress them.
    let sink = Arc::new(MemoryTraceSink::new(false));
    let owner = Arc::new(
        CheckerOwner::for_program_with_tracer(
            CheckerIdentity::new(Generation::new(counters), counters),
            counters,
            Arc::new(ProgramCheckerHost::new(program.clone())),
            Some(Tracer::new(sink.clone(), 0)),
        )
        .unwrap(),
    );
    let source = program.file(b"/main.ts").unwrap().source();
    let mut op = owner.operation().unwrap();
    op.begin_recursion_probe(None).unwrap();
    if iterative {
        op.use_iterative_expression_stack().unwrap();
    }
    let diagnostics = op.semantic_diagnostics(source).unwrap();
    let suggestions = op.recorded_suggestions(source).unwrap();
    let ty = op
        .get_type_at_location(initializer(program, b"probe"))
        .unwrap();
    let queried_type = op.type_to_string_default(ty).unwrap().as_bytes().to_vec();
    let probe = op.take_recursion_probe().unwrap();
    let expressions: Vec<_> = sink
        .events()
        .into_iter()
        .filter(|event| event.name == "checkExpression")
        .collect();
    assert!(
        !expressions.is_empty(),
        "expression trace must not be vacuous"
    );
    Observation {
        diagnostics,
        suggestions,
        queried_type,
        types: op.type_count(),
        symbols: op.symbol_count(),
        signatures: op.signature_count(),
        instantiations: op.instantiation_count(),
        expressions,
        maximum_expression_depth: probe["maximum_expression_depth"].as_u64().unwrap(),
    }
}

fn equivalent(text: &str) -> Observation {
    let (program, counters) = program(text.as_bytes());
    let recursive = observe(&program, &counters, false);
    let iterative = observe(&program, &counters, true);
    assert_eq!(iterative, recursive, "source: {text}");
    iterative
}

#[test]
fn operator_order_context_and_postchecks_match_recursive_checker() {
    for text in [
        "export const probe = (1 + 2) * (3 - 4) + 2 ** 3 ** 2;",
        "export const probe = missingLeft + missingRight;",
        "declare const a: string | undefined; export const probe = (a && missingAnd) || (a ?? missingNullish);",
        "export const probe = (missingCommaLeft, missingCommaRight);",
        "const enum E { A }; const invalid = ((E)); export const probe = E.A;",
        "declare function id<T>(x: T): T; declare function higher<T>(f: (x: T) => T, x: T): T; export const probe = higher(((id)), 1);",
        "declare function id<T>(x: T): T; const contextual: (x: number) => number = ((id)); export const probe = contextual(1);",
        "let x: number; const rhs = {x: 1}; ({x} = rhs); export const probe = x;",
        "let x: number; [x] = [1]; export const probe = x;",
        "declare const obj: {x: number}; obj.x += 1; export const probe = obj.x;",
        "const callback = (n: number) => n + 1; export const probe = (callback(1) + 2) * 3;",
        "const invalid = ({}) === ({}); export const probe = 1 + 2;",
    ] {
        equivalent(text);
    }
    let errors = equivalent("export const probe = missingLeft + missingRight;");
    assert_eq!(
        errors
            .diagnostics
            .iter()
            .map(|value| value.code)
            .collect::<Vec<_>>(),
        [2304, 2304]
    );
    assert!(errors.diagnostics[0].loc.pos() < errors.diagnostics[1].loc.pos());
}

#[test]
fn deep_binary_and_parenthesized_sources_keep_the_entire_logical_depth() {
    let chain = vec!["a"; 5001].join(" + ");
    let binary = equivalent(&format!(
        "declare const a: number; export const probe: number = {chain};"
    ));
    assert!(binary.diagnostics.is_empty());
    assert_eq!(binary.queried_type, b"number");
    assert!(binary.maximum_expression_depth >= 5001);
    let parentheses = equivalent(&format!(
        "export const probe = {}1{};",
        "(".repeat(5000),
        ")".repeat(5000)
    ));
    assert!(parentheses.diagnostics.is_empty());
    assert_eq!(parentheses.queried_type, b"1");
    assert!(parentheses.maximum_expression_depth >= 5001);
}

#[test]
fn rejected_foreign_query_does_not_change_later_expression_results() {
    // Public queries reject a foreign owner before expression entry. A malformed
    // frame's private MissingLink path needs a checker unit test, not this claim.
    let (program, counters) = program(b"export const probe = (1 + 2) * 3;");
    let (foreign, _) = self::program(b"export const probe = 1;");
    let owner = Arc::new(
        CheckerOwner::for_program(
            CheckerIdentity::new(Generation::new(&counters), &counters),
            &counters,
            Arc::new(ProgramCheckerHost::new(program.clone())),
        )
        .unwrap(),
    );
    let mut op = owner.operation().unwrap();
    op.begin_recursion_probe(None).unwrap();
    op.use_iterative_expression_stack().unwrap();
    assert_eq!(
        op.get_type_at_location(initializer(&foreign, b"probe")),
        Err(Error::Arena(tsr_arena::Error::WrongOwner))
    );
    let ty = op
        .get_type_at_location(initializer(&program, b"probe"))
        .unwrap();
    assert_eq!(op.type_to_string_default(ty).unwrap().as_bytes(), b"number");
    assert!(op
        .semantic_diagnostics(program.file(b"/main.ts").unwrap().source())
        .unwrap()
        .is_empty());
}
