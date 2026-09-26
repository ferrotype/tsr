//! Phase 2 C1.8: direct contracts of the checker foundations over production
//! entry points (docs/PHASE2-C1-plan.md). Each contract names its pinned
//! counterpart; where the pin's result was observed, it was observed with the
//! pinned `tsc` (`upstream/tsc/cmd/tsc`, `--noEmit --strict --target esnext`)
//! on the same program, and the expected diagnostics are the pin's.
//!
//! Contract 8 (the five relation modes over the 21 relater fixtures) is the
//! `scripts/s08_relater.py parity` step of the C1 exit and is not repeated
//! here.
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use tsr_arena::{CheckerIdentity, Counters, Generation, NodeId};
use tsr_ast::SyntaxKind as K;
use tsr_checker::{CheckerOwner, RelationKind};
use tsr_compiler::{FileCache, Program, ProgramCheckerHost, ProgramOptions};
use tsr_core::{CompilerOptions, ModuleKind, ScriptTarget, Tristate};
use tsr_jsstring::JsString;

fn program(files: &[(&str, &str)]) -> Arc<Program> {
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    for (name, text) in files {
        fs.insert_loaded(name.as_bytes(), text.as_bytes());
    }
    let roots = files
        .iter()
        .map(|(name, _)| JsString::from_bytes(name.as_bytes()))
        .collect();
    let options = CompilerOptions {
        target: ScriptTarget::ESNEXT,
        module: ModuleKind::ESNEXT,
        strict: Tristate::TRUE,
        ..Default::default()
    };
    Arc::new(
        Program::load(
            ProgramOptions {
                config: tsr_tsoptions::ParsedCommandLine::new(options, roots),
                host: Arc::new(tsr_bundled::BundledFs::new(Arc::new(fs.finish()))),
                current_directory: JsString::from_bytes(b"/".as_slice()),
                default_library_path: JsString::from_bytes(tsr_bundled::LIB_PATH),
                skip_module_resolution: false,
            },
            &mut FileCache::new(),
            &Counters::new(),
        )
        .unwrap(),
    )
}

fn checker(program: &Arc<Program>) -> (Counters, Generation, Arc<CheckerOwner>) {
    let counters = Counters::new();
    let generation = Generation::new(&counters);
    let owner = Arc::new(
        CheckerOwner::for_program(
            CheckerIdentity::new(generation.clone(), &counters),
            &counters,
            Arc::new(ProgramCheckerHost::new(program.clone())),
        )
        .unwrap(),
    );
    (counters, generation, owner)
}

/// The `VariableDeclaration` node named `name` in `file`.
fn declaration(program: &Program, file: &str, name: &str) -> NodeId {
    let file = program.file(file.as_bytes()).unwrap();
    let view = file.bound().view().ast();
    for statement in view
        .node_slice(view.node(file.source()).unwrap().statements(view).unwrap())
        .unwrap()
        .iter()
        .flatten()
    {
        let read = view.node(statement).unwrap();
        if read.kind() != K::VariableStatement {
            continue;
        }
        let list = read
            .as_variable_statement()
            .unwrap()
            .declaration_list()
            .unwrap();
        let list = view
            .node(list)
            .unwrap()
            .as_variable_declaration_list()
            .unwrap()
            .declarations()
            .unwrap();
        for declaration in view
            .node_slice(view.list(list).unwrap().nodes())
            .unwrap()
            .iter()
            .flatten()
        {
            let read = view.node(declaration).unwrap();
            if view.node_text(read.name().unwrap()).unwrap().as_bytes() == name.as_bytes() {
                return declaration;
            }
        }
    }
    panic!("missing declaration {name}");
}

fn name_of(program: &Program, file: &str, name: &str) -> NodeId {
    let declaration = declaration(program, file, name);
    let file = program.file(file.as_bytes()).unwrap();
    let view = file.bound().view().ast();
    view.node(declaration).unwrap().name().unwrap()
}

fn codes(diagnostics: &[tsr_ast::Diagnostic]) -> Vec<i32> {
    let mut codes: Vec<i32> = diagnostics.iter().map(|d| d.code).collect();
    codes.sort_unstable();
    codes
}

/// Contract 1 (`pushTypeResolution`/`popTypeResolution`, `getResolvedBaseConstraint`):
/// a resolution cycle is reported with the pin's diagnostics, and the checker
/// answers later queries. Pinned tsc on this program: TS2456, TS2313.
#[test]
fn lazy_resolution_reports_the_cycle_and_stays_usable() {
    let program = program(&[(
        "/cycle.ts",
        "type A = A;\nfunction f<T extends T>(x: T): T { return x; }\nlet n = 1;\n",
    )]);
    let (_counters, _generation, owner) = checker(&program);
    let mut op = owner.operation().unwrap();
    let source = program.file(b"/cycle.ts").unwrap().source();
    let diagnostics = op.semantic_diagnostics(source).unwrap();
    assert_eq!(codes(&diagnostics), vec![2313, 2456], "{diagnostics:?}");
    let n = name_of(&program, "/cycle.ts", "n");
    let ty = op.get_type_at_location(n).unwrap();
    assert_eq!(
        op.type_to_string(ty, 0).unwrap().as_bytes(),
        b"number",
        "a query after the cycle still resolves"
    );
    // The circular entities resolve to the error type, not to a fresh cycle.
    let again = op.semantic_diagnostics(source).unwrap();
    assert_eq!(
        codes(&again),
        vec![2313, 2456],
        "diagnostics are stable on repeat"
    );
}

/// Contract 2 (the relation fixtures' cold/repeated work classes): a query
/// repeated in the same checker returns the identical type and creates no
/// new type or signature.
#[test]
fn a_repeated_query_returns_the_same_type_without_new_work() {
    let program = program(&[(
        "/box.ts",
        "interface Box<T> { value: T; get(): T }\n\
         declare const b: Box<string>;\n\
         const v = b.get();\n",
    )]);
    let (_counters, _generation, owner) = checker(&program);
    let mut op = owner.operation().unwrap();
    let v = name_of(&program, "/box.ts", "v");
    let first = op.get_type_at_location(v).unwrap();
    let types = op.type_count();
    let signatures = op.signature_count();
    let second = op.get_type_at_location(v).unwrap();
    assert_eq!(first.id(), second.id(), "the same type id");
    assert_eq!(op.type_count(), types, "no type created by the repeat");
    assert_eq!(
        op.signature_count(),
        signatures,
        "no signature created by the repeat"
    );
    assert_eq!(op.type_to_string(second, 0).unwrap().as_bytes(), b"string");
}

/// Contract 3 (`Relation` caches, `maybeKeys`): a failed relation leaves no
/// success behind, and another mode is not answered from the first mode's
/// cache.
#[test]
fn a_failed_relation_commits_no_assumption_and_modes_keep_separate_caches() {
    let program = program(&[(
        "/rel.ts",
        "type A = { x: A; y: string };\n\
         type B = { x: B; y: number };\n\
         declare let a: A;\n\
         declare let b: B;\n",
    )]);
    let (_counters, _generation, owner) = checker(&program);
    let mut op = owner.operation().unwrap();
    let a = op
        .get_type_at_location(name_of(&program, "/rel.ts", "a"))
        .unwrap();
    let b = op
        .get_type_at_location(name_of(&program, "/rel.ts", "b"))
        .unwrap();
    assert!(!op
        .is_type_related_to(a, b, RelationKind::Assignable)
        .unwrap());
    let state = op.relation_state();
    let assignable = &state["caches"]["assignable"];
    let entries = assignable["entries"]
        .as_u64()
        .unwrap_or_else(|| panic!("no assignable cache in {state}"));
    assert!(entries >= 1, "{state}");
    for flag in assignable["result_flags"].as_array().unwrap() {
        let flag = flag.as_u64().unwrap();
        assert_eq!(flag & 1, 0, "no assignable entry records success: {state}");
    }
    assert_eq!(
        state["caches"]["comparable"]["entries"], 0,
        "the comparable cache is untouched by an assignability check: {state}"
    );
    // The same failing pair under comparability populates its own cache.
    assert!(!op
        .is_type_related_to(a, b, RelationKind::Comparable)
        .unwrap());
    let state = op.relation_state();
    assert!(
        state["caches"]["comparable"]["entries"].as_u64().unwrap() >= 1,
        "{state}"
    );
    // A relation that holds records success.
    assert!(op
        .is_type_related_to(a, a, RelationKind::Assignable)
        .unwrap());
    let string = op.builtin_type("stringType").unwrap();
    let object = op
        .get_type_at_location(name_of(&program, "/rel.ts", "a"))
        .unwrap();
    assert!(!op
        .is_type_related_to(string, object, RelationKind::Assignable)
        .unwrap());
}

/// Contract 4 (ADR 0007, checker-local merges): two checkers over one program
/// each merge the same interface into their own symbol, and both answer the
/// same declared type.
#[test]
fn two_checkers_merge_independently_over_one_program() {
    // Declarations in one file merge in the binder; the checker merges across
    // files, so the two halves live in two global scripts.
    let program = program(&[
        ("/a.ts", "interface I { a: string }\n"),
        ("/b.ts", "interface I { b: number }\ndeclare let i: I;\n"),
    ]);
    let (_c1, _g1, first) = checker(&program);
    let (_c2, _g2, second) = checker(&program);
    let mut op1 = first.operation().unwrap();
    let mut op2 = second.operation().unwrap();
    let node = name_of(&program, "/b.ts", "i");
    let t1 = op1.get_type_at_location(node).unwrap();
    let t2 = op2.get_type_at_location(node).unwrap();
    let mut p1: Vec<_> = op1
        .properties_of_type(t1)
        .unwrap()
        .into_iter()
        .map(|s| op1.symbol(s).unwrap().name_bytes().to_vec())
        .collect();
    let mut p2: Vec<_> = op2
        .properties_of_type(t2)
        .unwrap()
        .into_iter()
        .map(|s| op2.symbol(s).unwrap().name_bytes().to_vec())
        .collect();
    p1.sort();
    p2.sort();
    assert_eq!(p1, vec![b"a".to_vec(), b"b".to_vec()]);
    assert_eq!(p1, p2);
    let s1 = op1.type_symbol(t1).unwrap().unwrap();
    let s2 = op2.type_symbol(t2).unwrap().unwrap();
    assert_ne!(s1, s2, "each checker owns its merged symbol");
    // A type of one checker is refused by the other.
    assert!(op2
        .is_type_related_to(t1, t2, RelationKind::Assignable)
        .is_err());
}

fn deep_program(levels: usize) -> String {
    let mut text = String::from("type N0 = { x: string };\n");
    for i in 1..=levels {
        text.push_str(&format!("type N{i} = {{ x: N{} }};\n", i - 1));
    }
    text.push_str(&format!(
        "declare let a: N{levels};\nlet b: N{levels} = a;\n"
    ));
    text
}

/// Contract 5 (ADR 0011, `stacker::maybe_grow`): a relation deep enough to
/// grow the stack completes with the ordinary result on a small thread stack,
/// and the same checker answers a later query. Pinned tsc: no diagnostics.
#[test]
fn deep_relations_grow_the_stack_and_keep_the_checker_usable() {
    let text = deep_program(400);
    std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(move || {
            let program = program(&[("/deep.ts", text.as_str())]);
            let (_counters, _generation, owner) = checker(&program);
            let mut op = owner.operation().unwrap();
            let source = program.file(b"/deep.ts").unwrap().source();
            let diagnostics = op.semantic_diagnostics(source).unwrap();
            assert_eq!(codes(&diagnostics), Vec::<i32>::new(), "{diagnostics:?}");
            let b = name_of(&program, "/deep.ts", "b");
            let ty = op.get_type_at_location(b).unwrap();
            assert_eq!(op.type_to_string(ty, 0).unwrap().as_bytes(), b"N400");
        })
        .unwrap()
        .join()
        .unwrap();
}

/// Contract 6 (the pin's semantic limits): recursive structural types reach
/// the relater's 100-level backstop, which answers `TernaryMaybe` with no
/// diagnostic (relater.go:3133), so the assignment is accepted; an unbounded
/// instantiation reports `Type instantiation is excessively deep and
/// possibly infinite` (TS2589, checker.go:22452). Both observed with the
/// pinned tsc.
#[test]
fn semantic_limits_produce_the_pins_results() {
    let program = program(&[
        (
            "/recursive.ts",
            "type A = { x: A; y: string };\ntype B = { x: B; y: string };\ndeclare let a: A;\nlet b: B = a;\n",
        ),
        (
            "/depth.ts",
            "type Deep<T> = T extends any ? Deep<[T]> : never;\ntype X = Deep<string>;\n",
        ),
    ]);
    let (_counters, _generation, owner) = checker(&program);
    let mut op = owner.operation().unwrap();
    let recursive = program.file(b"/recursive.ts").unwrap().source();
    let diagnostics = op.semantic_diagnostics(recursive).unwrap();
    assert_eq!(codes(&diagnostics), Vec::<i32>::new(), "{diagnostics:?}");
    let depth = program.file(b"/depth.ts").unwrap().source();
    let diagnostics = op.semantic_diagnostics(depth).unwrap();
    assert_eq!(codes(&diagnostics), vec![2589], "{diagnostics:?}");
}

/// Contract 7 (ADR 0012): only an actually caught panic inside an operation
/// retires the generation; a fresh checker over the same program succeeds.
#[test]
fn an_injected_panic_retires_the_generation_and_a_fresh_checker_succeeds() {
    let program = program(&[("/p.ts", "declare let a: { x: string };\nlet b = a.x;\n")]);
    let (_counters, generation, owner) = checker(&program);
    let b = name_of(&program, "/p.ts", "b");
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut op = owner.operation().unwrap();
        op.get_type_at_location(b).unwrap();
        panic!("injected checker failure");
    }));
    assert!(result.is_err());
    assert_eq!(generation.validate(), Err(tsr_arena::Error::Retired));
    assert!(matches!(
        owner.operation(),
        Err(tsr_checker::Error::Arena(tsr_arena::Error::Retired))
    ));
    let (_counters, _generation, fresh) = checker(&program);
    let mut op = fresh.operation().unwrap();
    let ty = op.get_type_at_location(b).unwrap();
    assert_eq!(op.type_to_string(ty, 0).unwrap().as_bytes(), b"string");
}
