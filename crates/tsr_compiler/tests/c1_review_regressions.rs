//! Narrow C1 review regressions against the pinned Go compiler's diagnostics.
use std::sync::Arc;
use tsr_arena::{CheckerIdentity, Counters, Generation};
use tsr_checker::CheckerOwner;
use tsr_compiler::{FileCache, Program, ProgramCheckerHost, ProgramOptions};
use tsr_core::{CompilerOptions, ModuleKind, ScriptTarget, Tristate};
use tsr_jsstring::JsString;

fn program(files: &[(&str, &str)], skip_lib_check: bool) -> Arc<Program> {
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
        skip_lib_check: Tristate::from(skip_lib_check),
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

fn checker(program: &Arc<Program>) -> Arc<CheckerOwner> {
    let counters = Counters::new();
    let generation = Generation::new(&counters);
    Arc::new(
        CheckerOwner::for_program(
            CheckerIdentity::new(generation, &counters),
            &counters,
            Arc::new(ProgramCheckerHost::new(program.clone())),
        )
        .unwrap(),
    )
}

fn message(diagnostic: &tsr_ast::Diagnostic) -> String {
    String::from_utf8(tsr_compiler::diagnostic_writer::flattened(diagnostic, b"\n").unwrap())
        .unwrap()
}

#[test]
fn circular_imported_constraint_walks_each_nodes_owner() {
    let program = program(
        &[
            (
                "/a.ts",
                "import { f } from \"./b\"; const x: number = f(0);\n",
            ),
            (
                "/b.d.ts",
                "export declare function f<T extends T>(x: T): T;\n",
            ),
        ],
        true,
    );
    let owner = checker(&program);
    let mut op = owner.operation().unwrap();
    let source = program.file(b"/a.ts").unwrap().source();
    // Resolving f's constraint from the caller adds TS2751 related information
    // before skipLibCheck suppresses the declaration's circularity diagnostic.
    // The two ancestry walks therefore start in distinct retained AST owners.
    assert!(op.semantic_diagnostics(source).unwrap().is_empty());
    assert!(op.semantic_diagnostics(source).unwrap().is_empty());
}

#[test]
fn excess_property_filter_finds_object_inside_an_intersection() {
    let text = "const a: (object & {length: string}) | string = {length: 42};\n\
                const b: (object & {foo: number}) | string = {foo: undefined};\n";
    let program = program(&[("/excess.ts", text)], false);
    let owner = checker(&program);
    let mut op = owner.operation().unwrap();
    let diagnostics = op
        .semantic_diagnostics(program.file(b"/excess.ts").unwrap().source())
        .unwrap();
    assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    assert_eq!(diagnostics[0].code, 2322);
    assert_eq!(diagnostics[1].code, 2322);
    assert_eq!(
        message(&diagnostics[0]),
        "Type '{ length: number; }' is not assignable to type 'string | (object & { length: string; })'.\n  Types of property 'length' are incompatible.\n    Type 'number' is not assignable to type 'string'."
    );
    assert_eq!(
        message(&diagnostics[1]),
        "Type 'undefined' is not assignable to type 'number'."
    );
    assert_eq!(
        diagnostics[0].loc.pos(),
        i64::try_from(text.find("a:").unwrap()).unwrap()
    );
    assert_eq!(
        diagnostics[1].loc.pos(),
        i64::try_from(text.rfind("foo: undefined").unwrap()).unwrap()
    );
}

#[test]
fn mapped_target_diagnostics_follow_the_comparison_branch() {
    let text = "function attempted<K extends \"x\">(source: { x: number }) {\n\
                    const target: { [P in K]: string } = source;\n\
                }\n\
                function genericSource<K extends string>(source: { [P in K]: number }) {\n\
                    const target: { [P in K]: string } = source;\n\
                }\n\
                function excludeOptional<K extends string>(source: { [P in K]: number }) {\n\
                    const target: { [P in K]-?: string } = source;\n\
                }\n";
    let program = program(&[("/mapped.ts", text)], false);
    let owner = checker(&program);
    let mut op = owner.operation().unwrap();
    let diagnostics = op
        .semantic_diagnostics(program.file(b"/mapped.ts").unwrap().source())
        .unwrap();
    assert_eq!(diagnostics.len(), 3, "{diagnostics:?}");
    assert!(diagnostics.iter().all(|diagnostic| diagnostic.code == 2322));
    assert_eq!(
        diagnostics.iter().map(message).collect::<Vec<_>>(),
        [
            "Type '{ x: number; }' is not assignable to type '{ [P in K]: string; }'.",
            "Type '{ [P in K]: number; }' is not assignable to type '{ [P in K]: string; }'.\n  Type 'number' is not assignable to type 'string'.",
            "Type '{ [P in K]: number; }' is not assignable to type '{ [P in K]-?: string; }'.",
        ]
    );
}
