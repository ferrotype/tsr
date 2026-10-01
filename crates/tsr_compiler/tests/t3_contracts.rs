//! Phase 3 T3 contracts (docs/PHASE3-plan.md, sections 4 and 5): the
//! transform arena and the emit side tables are released with each file's
//! emit and leave the source arena unchanged, and the TypeScript transforms
//! carry deep inputs through their growth guards.
#[allow(dead_code)]
#[path = "support/phase3_contracts.rs"]
mod support;

use std::sync::{Mutex, MutexGuard, PoisonError};
use support::{files, Mode, LIB};
use tsr_arena::{Counters, Counts};
use tsr_checker::CheckerRequest;
use tsr_compiler::{CheckedProgram, EmitOptions, Program, WriteFileData};
use tsr_core::{CompilerOptions, ModuleKind, NewLineKind, ScriptTarget, Tristate};
use tsr_printer::{EmitContext, Printer, PrinterOptions};

/// Every allocation of this test binary, so a test can read the live heap.
#[global_allocator]
static ALLOCATOR: cap::Cap<std::alloc::System> = cap::Cap::new(std::alloc::System, usize::MAX);

/// The live heap is process-wide: the tests of this binary run one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A file that every TypeScript transform of the chain rewrites: type
/// annotations and a type-only import to erase, enums, namespaces and
/// parameter properties to lower.
fn transformed_file(index: usize) -> String {
    let mut text = String::from("import type { Shape } from \"./shapes\";\n");
    for i in 0..12 {
        text.push_str(&format!(
            "export enum E{i} {{ A = {i}, B = A + 1, C = B * 2 }}\n\
             export namespace N{i} {{ export const v: number = E{i}.C; export function f(s: Shape): number {{ return s.size + v; }} }}\n\
             export class C{i} {{ constructor(public a: number, private b: Shape) {{}} get size(): number {{ return this.a + this.b.size; }} }}\n"
        ));
    }
    text.push_str(&format!(
        "export const file{index}: string = \"f{index}\";\n"
    ));
    text
}

/// Eight files of the same shape and size beside the declaration file they
/// import a type from.
fn retention_program() -> Vec<(String, String)> {
    let mut program = files(&[
        ("/lib.d.ts", LIB),
        ("/shapes.d.ts", "export interface Shape { size: number }\n"),
    ]);
    for index in 1..=8 {
        program.push((format!("/f{index}.ts"), transformed_file(index)));
    }
    program
}

fn esnext() -> CompilerOptions {
    CompilerOptions {
        target: ScriptTarget::ESNEXT,
        module: ModuleKind::ESNEXT,
        ..CompilerOptions::default()
    }
}

/// Each source file printed back as itself, which reads every node of its
/// arena.
fn reprints(program: &Program) -> Vec<Vec<u8>> {
    program
        .files()
        .iter()
        .map(|file| {
            let view = file.bound().view();
            let context = EmitContext::new();
            let mut printer = Printer::new(PrinterOptions::default(), &context);
            printer
                .emit_source_file(view.ast(), file.source())
                .expect("the source prints")
        })
        .collect()
}

/// One emit whose write callback records the live heap at each script
/// write, without allocating while it records: the result is dropped before
/// the emit's caller reads the heap again.
fn measured_emit(checked: &CheckedProgram, live_at_writes: &Mutex<Vec<usize>>) -> usize {
    live_at_writes.lock().expect("recorder").clear();
    let write_file = |name: &[u8], _: &[u8], _: &mut WriteFileData| {
        if name.ends_with(b".js") {
            let live = ALLOCATOR.allocated();
            live_at_writes.lock().expect("recorder").push(live);
        }
        Ok(())
    };
    let options = EmitOptions {
        write_file: Some(&write_file),
        ..EmitOptions::default()
    };
    let result = checked
        .emit(&CheckerRequest::default(), &options)
        .expect("the emit succeeds")
        .expect("an emit result");
    let emitted = result.emitted_files.len();
    drop(result);
    emitted
}

/// The transform arena and the emit side tables are released with each
/// file's emit (plan T3: "one `AstBuilder` per emitted file for the script
/// chain, released after printing"; section 5: "a transform arena and emit
/// side tables released per file"), and the source arena is unchanged:
///
/// - single-threaded, after a warm-up emit fills the checkers' caches, an
///   emit leaves the live heap exactly as it found it once its result is
///   dropped, three times over, so nothing of a file's transformation (its
///   arena, the emit context's side tables, the name generator, the
///   printer, the source-map generator) outlives the emit;
/// - while the eight files are written, the live heap at each script write
///   stays within a quarter of what one file's transformation holds at its
///   own write, so each file's transformation is released before the next
///   file's emit, not at the end of the group;
/// - in both modes, the program's arena counters do not move during an emit
///   and every source file prints back the same bytes afterwards; and once
///   the program is dropped every owner it counted is released, so no emit
///   kept a file's storage alive.
#[test]
fn a_files_transformation_is_released_with_its_emit_and_the_source_is_unchanged() {
    let _serial = serial();
    let program_files = retention_program();
    for mode in Mode::BOTH {
        let counters = Counters::new();
        let program = support::load(&program_files, &esnext(), mode, &counters);
        let checked = CheckedProgram::new(program.clone(), &counters, None);
        let live_at_writes = Mutex::new(Vec::with_capacity(64));
        // Warm-up: the checkers' caches and the program's lazily computed
        // data are filled by the first emit and kept by the program.
        assert_eq!(measured_emit(&checked, &live_at_writes), 8);
        let sources = reprints(&program);
        let arena = counters.snapshot();
        for round in 0..3 {
            let before = ALLOCATOR.allocated();
            assert_eq!(measured_emit(&checked, &live_at_writes), 8);
            let after = ALLOCATOR.allocated();
            assert_eq!(counters.snapshot(), arena, "{mode:?} round {round}");
            if mode == Mode::Single {
                // A parallel group's threads release their thread-locals
                // after the group returns, so only this mode is exact.
                assert_eq!(after, before, "{mode:?} round {round}: retained bytes");
                let live = live_at_writes.lock().expect("recorder").clone();
                assert_eq!(live.len(), 8);
                let held = live[0] - before;
                let spread = live.iter().max().unwrap() - live.iter().min().unwrap();
                assert!(
                    spread < held / 4,
                    "round {round}: the live heap grows by {spread} bytes over the files' writes; \
                     one file's transformation holds {held}"
                );
            }
        }
        assert!(
            reprints(&program) == sources,
            "{mode:?}: the sources changed"
        );
        drop(checked);
        drop(program);
        assert_eq!(counters.snapshot(), Counts::default(), "{mode:?}");
    }
}

/// Depth of the deep inputs through the transforms. Every transform reads a
/// source node through `AstBuilder::factory_view`, which walks the node's
/// parent chain to its source file, so the cost is quadratic in the depth.
const DEPTH: usize = 500;

/// The corpus's own stress row, `binderBinaryExpressionStress.ts` (a
/// 1,499-term chain), emitted with its options, prints the pinned
/// reference baseline's JavaScript byte for byte. The transforms' quadratic
/// reads make it take about a minute per mode in a debug build, so only the
/// release run emits it.
fn stress_row() {
    if cfg!(debug_assertions) {
        return;
    }
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../upstream/tsc/testdata");
    let source = std::fs::read_to_string(format!(
        "{root}/tests/cases/compiler/binderBinaryExpressionStress.ts"
    ))
    .expect("the stress row");
    let baseline = std::fs::read(format!(
        "{root}/baselines/reference/compiler/binderBinaryExpressionStress.js"
    ))
    .expect("its JavaScript baseline");
    let header: &[u8] = b"//// [binderBinaryExpressionStress.js]\r\n";
    let start = baseline
        .windows(header.len())
        .position(|window| window == header)
        .expect("the baseline's script section")
        + header.len();
    let options = CompilerOptions {
        target: ScriptTarget::ESNEXT,
        remove_comments: Tristate::TRUE,
        new_line: NewLineKind::CRLF,
        ..CompilerOptions::default()
    };
    let observed = support::emit_deep(
        &files(&[("/binderBinaryExpressionStress.ts", &source)]),
        &options,
    );
    assert!(
        observed.text("/binderBinaryExpressionStress.js") == &baseline[start..],
        "the stress row's emit differs from its baseline"
    );
}

/// Deep inputs through the TypeScript transforms (type eraser, import
/// elision, runtime syntax, legacy decorators) and the use-strict and
/// implied-module transforms (ADR 0011; plan section 6 "Recursion"): the
/// corpus's stress row against its baseline (release only); `DEPTH` levels
/// of a left-nested binary chain, nested parentheses, property-access and
/// call chains, nested blocks, functions and arrow functions; half that of
/// nested namespaces, classes, and classes with parameter properties (the
/// runtime-syntax transform visits a constructor's body directly); an eighth
/// of nested decorated classes (the legacy-decorator transform walks each
/// class for static self-references, so the walk is quadratic). Each is
/// emitted single-threaded on a 256 KiB thread and concurrently on the work
/// group's reserved stacks with the same output, and the whole depth is
/// transformed.
#[test]
fn deep_inputs_transform_through_the_growth_guards() {
    let _serial = serial();
    stress_row();
    let n = DEPTH;
    let program = |text: String| files(&[("/lib.d.ts", LIB), ("/a.ts", &text)]);
    let emit = |text: String, options: &CompilerOptions| {
        support::emit_deep(&program(text), options)
            .text("/a.js")
            .to_vec()
    };
    let options = esnext();
    let script = |declarations: &str, expression: String| {
        format!("{declarations}export const x: number = {expression};\n")
    };

    let binary = emit(
        script("declare const a: number;\n", support::repeat("a", n, " + ")),
        &options,
    );
    assert_eq!(support::occurrences(&binary, "a + "), n - 1);
    let parentheses = emit(
        script(
            "declare const a: number;\n",
            format!("{}a{}", "(".repeat(n), ")".repeat(n)),
        ),
        &options,
    );
    assert_eq!(support::occurrences(&parentheses, "("), n);
    let property = emit(
        script("declare const a: any;\n", format!("a{}", ".b".repeat(n))),
        &options,
    );
    assert_eq!(support::occurrences(&property, ".b"), n);
    let call = emit(
        script("declare const a: any;\n", format!("a{}", "()".repeat(n))),
        &options,
    );
    assert_eq!(support::occurrences(&call, "()"), n);
    let blocks = emit(
        format!(
            "export function f(): void {{ {}let v: number = 1;{} }}\n",
            "{".repeat(n),
            "}".repeat(n)
        ),
        &options,
    );
    assert_eq!(support::occurrences(&blocks, "{"), n + 1);
    let functions = emit(
        format!(
            "export {}{}\n",
            support::numbered(n, |i| format!("function f{i}(p: number): void {{ ")),
            "} ".repeat(n)
        ),
        &options,
    );
    assert_eq!(support::occurrences(&functions, "(p) {"), n);
    let arrows = emit(
        format!("export const f = {}1;\n", "(p: number) => ".repeat(n)),
        &options,
    );
    assert_eq!(support::occurrences(&arrows, "(p) => "), n);
    let namespaces_depth = n / 2;
    let namespaces = emit(
        format!(
            "{}{}",
            support::numbered(namespaces_depth, |i| format!(
                "export namespace N{i} {{ export const v: number = {i}; "
            )),
            "} ".repeat(namespaces_depth)
        ),
        &options,
    );
    assert_eq!(
        support::occurrences(&namespaces, "(function (N"),
        namespaces_depth
    );
    let class_depth = n / 2;
    let classes = emit(
        format!(
            "export function g(): void {{ {}{} }}\n",
            support::numbered(class_depth, |i| format!(
                "class C{i} {{ m(p: number): void {{ "
            )),
            "} } ".repeat(class_depth)
        ),
        &options,
    );
    assert_eq!(support::occurrences(&classes, "m(p) {"), class_depth);
    let properties_depth = n / 2;
    let parameter_properties = emit(
        format!(
            "export function g(): void {{ {}{} }}\n",
            support::numbered(properties_depth, |i| format!(
                "class C{i} {{ constructor(public p: number) {{ "
            )),
            "} } ".repeat(properties_depth)
        ),
        &options,
    );
    assert_eq!(
        support::occurrences(&parameter_properties, "this.p = p;"),
        properties_depth
    );
    let decorated_depth = n / 8;
    let decorated = emit(
        format!(
            "declare const d: any;\nexport function g(): void {{ {}{} }}\n",
            support::numbered(decorated_depth, |i| format!(
                "@d class C{i} {{ @d m(): void {{ "
            )),
            "} } ".repeat(decorated_depth)
        ),
        &CompilerOptions {
            experimental_decorators: Tristate::TRUE,
            ..esnext()
        },
    );
    assert_eq!(
        support::occurrences(&decorated, "__decorate(["),
        2 * decorated_depth
    );
}

/// Depth of the nested destructuring patterns.
const PATTERN_DEPTH: usize = 300;

/// A declared object destructuring `PATTERN_DEPTH` patterns deep with a rest
/// element at the bottom. At ESNext the runtime-syntax transform's
/// `recordDeclarationInScope` and the declaration transform's binding-name
/// walks (`getBindingNameVisible`, its binding-name visitor) recurse once per
/// nested pattern; at ES2017 the destructuring flattener
/// (`flattenBindingOrAssignmentElement`) and its pattern walks
/// (`bindingOrAssignmentElementAssignsToName`,
/// `bindingOrAssignmentElementContainsNonLiteralComputedName`) do too. The
/// checker's visibility walk takes a deeper pattern to overflow; the T7
/// contracts declare one. Each target is emitted single-threaded on a 256
/// KiB thread and concurrently on the work group's reserved stacks with the
/// same output, and the whole depth is kept, or lowered to one property
/// access chain and one `__rest`.
#[test]
fn deep_destructuring_patterns_transform_through_the_growth_guards() {
    let _serial = serial();
    let n = PATTERN_DEPTH;
    let text = format!(
        "declare const s: any;\nexport const {{ {}a, ...r{} }} = s;\n",
        "a: { ".repeat(n),
        " }".repeat(n)
    );
    for target in [ScriptTarget::ESNEXT, ScriptTarget::ES2017] {
        let options = CompilerOptions {
            target,
            declaration: Tristate::TRUE,
            ..esnext()
        };
        let observed =
            support::emit_deep(&files(&[("/lib.d.ts", LIB), ("/a.ts", &text)]), &options);
        let script = observed.text("/a.js");
        if target == ScriptTarget::ESNEXT {
            assert_eq!(support::occurrences(script, "a: { "), n);
        } else {
            let chain = format!("(_a = s{}, _a)", ".a".repeat(n));
            assert_eq!(support::occurrences(script, &chain), 1);
            assert_eq!(support::occurrences(script, "r = __rest(_a, [\"a\"])"), 1);
        }
        assert_eq!(support::occurrences(observed.text("/a.d.ts"), "a: { "), n);
    }
}
