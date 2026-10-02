//! Phase 3 T7 contracts (docs/PHASE3-plan.md, sections 4 and 5): the
//! declaration transform's arena is released with each file's emit, and
//! declaration emit carries deep type nesting through its growth guards.
#[allow(dead_code)]
#[path = "support/phase3_contracts.rs"]
mod support;

use std::sync::{Mutex, MutexGuard, PoisonError};
use support::{files, Mode, LIB};
use tsr_arena::{Counters, Counts};
use tsr_checker::CheckerRequest;
use tsr_compiler::{CheckedProgram, EmitOnly, EmitOptions, WriteFileData};
use tsr_core::{CompilerOptions, ModuleKind, ScriptTarget, Tristate};

/// Every allocation of this test binary, so a test can read the live heap.
#[global_allocator]
static ALLOCATOR: cap::Cap<std::alloc::System> = cap::Cap::new(std::alloc::System, usize::MAX);

/// The live heap is process-wide: the tests of this binary run one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

fn declarations() -> CompilerOptions {
    CompilerOptions {
        target: ScriptTarget::ESNEXT,
        module: ModuleKind::ESNEXT,
        declaration: Tristate::TRUE,
        ..CompilerOptions::default()
    }
}

/// A file whose declaration emit serializes inferred types (a variable's,
/// a property's, an accessor's, a function's and a generic function's
/// return type), keeps and rewrites annotated ones, declares an expando
/// function's namespace and elides implementations. A function's return
/// type and an expando namespace are serialized in scopes the checker makes
/// for the request; repeated emits reuse them and leave the checker as the
/// first emit left it.
fn declared_file(index: usize) -> String {
    let mut text = String::new();
    for i in 0..12 {
        text.push_str(&format!(
            "export interface I{i} {{ a: number; b: string[]; c?: I{i} }}\n\
             export const v{i} = {{ a: {i}, b: [\"x\"], nested: {{ deep: [{i}, {i}] }} }};\n\
             export function f{i}(p: I{i}, q: number) {{ return {{ p, size: p.a + q }}; }}\n\
             export function g{i}<T extends I{i}>(t: T, u: T[]) {{ return [t, ...u]; }}\n\
             export function e{i}(): void {{}}\ne{i}.tag = {i};\n\
             export class C{i} {{ private hidden = {i}; constructor(public shown: I{i}) {{}} get both() {{ return [this.hidden, this.shown]; }} }}\n\
             export type T{i} = {{ [K in keyof I{i}]: I{i}[K] extends number ? K : never }};\n"
        ));
    }
    text.push_str(&format!("export const file{index} = \"f{index}\";\n"));
    text
}

/// One `EmitOnly::Dts` emit whose write callback records the live heap at
/// each declaration write, without allocating while it records; the result
/// is dropped before the caller reads the heap again.
fn measured_declaration_emit(
    checked: &CheckedProgram,
    live_at_writes: &Mutex<Vec<usize>>,
) -> usize {
    live_at_writes.lock().expect("recorder").clear();
    let write_file = |name: &[u8], _: &[u8], _: &mut WriteFileData| {
        if name.ends_with(b".d.ts") {
            let live = ALLOCATOR.allocated();
            live_at_writes.lock().expect("recorder").push(live);
        }
        Ok(())
    };
    let options = EmitOptions {
        write_file: Some(&write_file),
        emit_only: EmitOnly::Dts,
        ..EmitOptions::default()
    };
    let result = checked
        .emit(&CheckerRequest::default(), &options)
        .expect("the emit succeeds")
        .expect("an emit result");
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let emitted = result.emitted_files.len();
    drop(result);
    emitted
}

/// The declaration transform's arena and emit context are released with
/// each file's declaration emit (`emitDeclarationFile` builds both for the
/// file; the plan's T3 retention contract, which the declaration transform
/// follows): single-threaded, after a warm-up emit fills the checkers'
/// caches, a declaration-only emit leaves the live heap exactly as it found
/// it once its result is dropped, three times over; the live heap at each
/// declaration write stays within a quarter of what one file's declaration
/// emit holds at its own write; in both modes the program's arena counters
/// do not move during an emit, and once the program is dropped every owner
/// it counted is released.
#[test]
fn the_declaration_transform_is_released_with_each_files_emit() {
    let _serial = serial();
    let mut program_files = files(&[("/lib.d.ts", LIB)]);
    for index in 1..=8 {
        program_files.push((format!("/f{index}.ts"), declared_file(index)));
    }
    for mode in Mode::BOTH {
        let counters = Counters::new();
        let program = support::load(&program_files, &declarations(), mode, &counters);
        let checked = CheckedProgram::new(program.clone(), &counters, None);
        let live_at_writes = Mutex::new(Vec::with_capacity(64));
        assert_eq!(measured_declaration_emit(&checked, &live_at_writes), 8);
        let arena = counters.snapshot();
        for round in 0..3 {
            let before = ALLOCATOR.allocated();
            assert_eq!(measured_declaration_emit(&checked, &live_at_writes), 8);
            let after = ALLOCATOR.allocated();
            assert_eq!(counters.snapshot(), arena, "{mode:?} round {round}");
            if mode == Mode::Single {
                // A parallel group's threads release their thread-locals
                // after the group returns, so only this mode is exact.
                assert_eq!(after, before, "round {round}: retained bytes");
                let live = live_at_writes.lock().expect("recorder").clone();
                assert_eq!(live.len(), 8);
                let held = live[0] - before;
                let spread = live.iter().max().unwrap() - live.iter().min().unwrap();
                assert!(
                    spread < held / 4,
                    "round {round}: the live heap grows by {spread} bytes over the files' writes; \
                     one file's declaration emit holds {held}"
                );
            }
        }
        drop(checked);
        drop(program);
        assert_eq!(counters.snapshot(), Counts::default(), "{mode:?}");
    }
}

/// Depth of the deep type nesting.
const DEPTH: usize = 400;

/// Deep type nesting through declaration emit (ADR 0011; the declaration
/// transformer visits a type node's children through its own visitor, once
/// per level), `DEPTH` levels deep: a nested object type literal, a nested
/// generic type reference, a nested conditional type, an array literal whose
/// inferred type the checker widens and the node builder serializes (the
/// checker's widening and `isConstContext` recurse once per level), and half
/// that of nested namespaces exporting types. Each is emitted
/// single-threaded on a 256 KiB thread and concurrently on the work group's
/// reserved stacks with the same output, and the whole depth is declared.
#[test]
fn deep_type_nesting_declares_through_the_growth_guards() {
    let _serial = serial();
    let n = DEPTH;
    let declare = |text: String| {
        support::emit_deep(
            &files(&[("/lib.d.ts", LIB), ("/a.ts", &text)]),
            &declarations(),
        )
        .text("/a.d.ts")
        .to_vec()
    };
    let object = declare(format!(
        "export type T = {}number{};\n",
        "{ a: ".repeat(n),
        " }".repeat(n)
    ));
    assert_eq!(support::occurrences(&object, "a: "), n);
    let reference = declare(format!(
        "export type T = {}number{};\n",
        "Array<".repeat(n),
        ">".repeat(n)
    ));
    assert_eq!(support::occurrences(&reference, "Array<"), n);
    let conditional = declare(format!(
        "export type T<X> = {}never;\n",
        "X extends string ? 1 : ".repeat(n)
    ));
    assert_eq!(
        support::occurrences(&conditional, "X extends string ? 1 : "),
        n
    );
    let inferred = declare(format!(
        "export const v = {}1{};\n",
        "[".repeat(n),
        "]".repeat(n)
    ));
    assert_eq!(
        String::from_utf8_lossy(&inferred),
        format!("export declare const v: number{};\n", "[]".repeat(n))
    );
    let namespace_depth = n / 2;
    let namespaces = declare(format!(
        "{}{}",
        support::numbered(namespace_depth, |i| format!(
            "export namespace N{i} {{ export type T = {i}; "
        )),
        "} ".repeat(namespace_depth)
    ));
    assert_eq!(
        support::occurrences(&namespaces, "type T = "),
        namespace_depth
    );
}

/// Depth of the nested binding patterns declared.
const PATTERN_DEPTH: usize = 600;

/// A declared object destructuring `PATTERN_DEPTH` patterns deep: the
/// declaration transform's binding walks (`hasAnyBindingInitializers`,
/// `getBindingNameVisible`, its binding-name visitor) recurse once per
/// nested pattern, and the checker answers the innermost binding element's
/// visibility by asking for each enclosing pattern's declaration in turn
/// (`isDeclarationVisible`), about 4 KiB of stack per level in a debug
/// build, so this takes twice the T3 contract's depth to exceed the segment
/// an outer guard grows into. Emitted single-threaded on a 256 KiB thread
/// and concurrently on the work group's reserved stacks with the same
/// output; the whole depth is declared.
#[test]
fn deep_binding_patterns_declare_through_the_growth_guards() {
    let _serial = serial();
    let n = PATTERN_DEPTH;
    let text = format!(
        "declare const s: any;\nexport const {{ {}a, ...r{} }} = s;\n",
        "a: { ".repeat(n),
        " }".repeat(n)
    );
    let observed = support::emit_deep(
        &files(&[("/lib.d.ts", LIB), ("/a.ts", &text)]),
        &declarations(),
    );
    assert_eq!(support::occurrences(observed.text("/a.d.ts"), "a: { "), n);
}
