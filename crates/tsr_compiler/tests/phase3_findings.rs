//! Fixtures the Phase 3 contract suites (`t1_contracts.rs` to
//! `t8_contracts.rs`) found failing or too slow to run as contracts, kept
//! ignored with the reason so they can be run by name (`--ignored`). Each
//! states what it shows; none is a contract witness.
#[allow(dead_code)]
#[path = "support/phase3_contracts.rs"]
mod support;

use support::{files, Mode, LIB};
use tsr_arena::Counters;
use tsr_checker::CheckerRequest;
use tsr_compiler::{CheckedProgram, EmitOnly, EmitOptions, WriteFileData};
use tsr_core::{CompilerOptions, ModuleKind, ScriptTarget, Tristate};

fn options(target: ScriptTarget) -> CompilerOptions {
    CompilerOptions {
        target,
        module: ModuleKind::ESNEXT,
        declaration: Tristate::TRUE,
        ..CompilerOptions::default()
    }
}

/// A 600-level nested object literal whose inferred type is declared. The
/// checker's contextual typing (`getContextualType`) and `isConstContext`
/// walk up the literal once per level from every level, as the pin's do;
/// without their growth guards the walk overflows the 2 MiB segment the
/// checker's expression guard grows into (a debug build at this depth). With
/// the guards it passes, in about 50 seconds per mode in a debug build, so
/// it is not run with the T7 contracts.
#[test]
#[ignore = "about 50 s per mode in a debug build (the contextual-type walks are quadratic, as the pin's)"]
fn deep_inferred_object_literal_declares_through_the_growth_guards() {
    let n = 600;
    let text = format!(
        "export const v = {}1{};\n",
        "{ a: ".repeat(n),
        " }".repeat(n)
    );
    let observed = support::emit_deep(
        &files(&[("/lib.d.ts", LIB), ("/a.ts", &text)]),
        &options(ScriptTarget::ESNEXT),
    );
    assert_eq!(support::occurrences(observed.text("/a.d.ts"), "a: "), n);
}

/// A 300-level nested destructuring pattern. The runtime-syntax transform's
/// `recordDeclarationInScope` (ESNext) and the destructuring flattener's
/// pattern walks (`bindingOrAssignmentElementAssignsToName`,
/// `bindingOrAssignmentElementContainsNonLiteralComputedName`, at ES2017)
/// recurse once per nested pattern without a growth guard, and overflow the
/// 256 KiB stack the contracts emit on.
#[test]
#[ignore = "finding: overflows; the destructuring walks have no growth guard"]
fn deep_destructuring_patterns_transform_through_the_growth_guards() {
    let n = 300;
    let text = format!(
        "declare const s: any;\nexport const {{ {}a, ...r{} }} = s;\n",
        "a: { ".repeat(n),
        " }".repeat(n)
    );
    for target in [ScriptTarget::ESNEXT, ScriptTarget::ES2017] {
        let observed = support::emit_deep(
            &files(&[("/lib.d.ts", LIB), ("/a.ts", &text)]),
            &options(target),
        );
        assert!(!observed.text("/a.js").is_empty());
    }
}

/// 300 nested blocks with a `using` declaration each, at ES2022. The `using`
/// transform visits a block's statements directly, once per nested block,
/// without a growth guard, and overflows the 256 KiB stack the contracts
/// emit on; it also takes over two minutes at this depth in a debug build.
#[test]
#[ignore = "finding: overflows; usingDeclarationTransformer.visit has no growth guard"]
fn deep_using_blocks_transform_through_the_growth_guards() {
    let n = 300;
    let text = format!(
        "export function f(): void {{ {}{} }}\n",
        "{ using a = null; ".repeat(n),
        "}".repeat(n)
    );
    let observed = support::emit_deep(
        &files(&[("/lib.d.ts", LIB), ("/a.ts", &text)]),
        &options(ScriptTarget::ES2022),
    );
    assert_eq!(
        support::occurrences(observed.text("/a.js"), "= __addDisposableResource(env_"),
        n
    );
}

/// Repeated declaration emits of one program keep the checker as it was
/// after the first: the twelve functions here infer their return types,
/// and each declaration emit adds checker storage (its counted allocations
/// grow by one to three pages and the live heap by 12 to 140 KiB per emit,
/// without bound), though the checker's type, symbol and signature counts do
/// not change. Functions that annotate their return types, inferred
/// variable, property and accessor types, classes and mapped types do not
/// grow it.
#[test]
#[ignore = "finding: each declaration emit of an inferred function return type grows the checker"]
fn declaration_emit_of_inferred_return_types_keeps_no_checker_state() {
    let mut text = String::new();
    for i in 0..12 {
        text.push_str(&format!(
            "export interface I{i} {{ a: number }}\nexport function f{i}(p: I{i}, q: number) {{ return q; }}\n"
        ));
    }
    let program_files = files(&[("/lib.d.ts", LIB), ("/a.ts", &text)]);
    let counters = Counters::new();
    let program = support::load(
        &program_files,
        &options(ScriptTarget::ESNEXT),
        Mode::Single,
        &counters,
    );
    let checked = CheckedProgram::new(program, &counters, None);
    let write_file = |_: &[u8], _: &[u8], _: &mut WriteFileData| Ok(());
    let emit = || {
        checked
            .emit(
                &CheckerRequest::default(),
                &EmitOptions {
                    write_file: Some(&write_file),
                    emit_only: EmitOnly::Dts,
                    ..EmitOptions::default()
                },
            )
            .expect("the emit succeeds")
            .expect("a result");
    };
    emit();
    let after_first = counters.snapshot();
    for _ in 0..10 {
        emit();
    }
    assert_eq!(counters.snapshot(), after_first);
}
