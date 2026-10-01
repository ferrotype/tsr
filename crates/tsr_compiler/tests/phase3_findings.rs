//! Fixtures the Phase 3 contract suites (`t1_contracts.rs` to
//! `t8_contracts.rs`) found too slow to run as contracts, kept ignored with
//! the reason so they can be run by name (`--ignored`). Each states what it
//! shows; none is a contract witness. The deep destructuring, nested `using`
//! and repeated declaration emit findings are fixed and now run as the T3,
//! T5 and T7 contracts.
#[allow(dead_code)]
#[path = "support/phase3_contracts.rs"]
mod support;

use support::{files, LIB};
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
