//! Phase 3 T4 contracts (docs/PHASE3-plan.md, sections 4 and 5): the module
//! transforms' helpers belong to the file that needs them, and the module
//! and inliner transforms carry deep chains through their growth guards.
#[allow(dead_code)]
#[path = "support/phase3_contracts.rs"]
mod support;

use support::{files, Mode, LIB};
use tsr_core::{CompilerOptions, ModuleKind, ScriptTarget, Tristate};

fn commonjs() -> CompilerOptions {
    CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::COMMON_JS,
        es_module_interop: Tristate::TRUE,
        ..CompilerOptions::default()
    }
}

const MODULE: &str =
    "export default function main(): number { return 1; }\nexport const value: number = 2;\n";
const PLAIN: &str =
    "export const plain: number = 3;\nexport function twice(p: number): number { return p * 2; }\n";
const INTEROP: &str = "import main from \"./m\";\nimport * as all from \"./m\";\nexport * from \"./m\";\nexport const both: number = main() + all.value;\n";

/// The helpers the CommonJS transform requests (`__importDefault`,
/// `__importStar`, `__exportStar` and `__createBinding` under
/// `esModuleInterop`) are recorded in the emit context of the file that
/// needs them and printed only there: in a program whose middle file needs
/// them (the single-threaded group emits the last file first, so a file
/// emitted after it precedes it), the other files print exactly what they
/// print in a program without that file, in both modes.
#[test]
fn module_helpers_belong_to_the_file_that_needs_them() {
    let helpers = [
        "__importDefault",
        "__importStar",
        "__exportStar",
        "__createBinding",
    ];
    let alone = files(&[
        ("/lib.d.ts", LIB),
        ("/m.ts", MODULE),
        ("/a.ts", PLAIN),
        ("/c.ts", PLAIN),
    ]);
    let with_interop = files(&[
        ("/lib.d.ts", LIB),
        ("/m.ts", MODULE),
        ("/a.ts", PLAIN),
        ("/b.ts", INTEROP),
        ("/c.ts", PLAIN),
    ]);
    for mode in Mode::BOTH {
        let (checked, _) = support::checked(&alone, &commonjs(), mode);
        let expected = support::emit_all(&checked);
        let (checked, _) = support::checked(&with_interop, &commonjs(), mode);
        let observed = support::emit_all(&checked);
        let interop = String::from_utf8_lossy(observed.text("/b.js")).into_owned();
        for helper in helpers {
            assert!(interop.contains(helper), "{mode:?}: /b.js lacks {helper}");
        }
        for name in ["/m.js", "/a.js", "/c.js"] {
            assert_eq!(
                observed.text(name),
                expected.text(name),
                "{mode:?}: {name} changed beside a file that needs helpers"
            );
            for helper in helpers {
                assert_eq!(
                    support::occurrences(observed.text(name), helper),
                    0,
                    "{mode:?}: {name} prints {helper}"
                );
            }
        }
    }
}

/// Depth of the deep chains.
const DEPTH: usize = 500;

/// Deep property-access and call chains through the module and inliner
/// transforms (ADR 0011), `DEPTH` links each, with CommonJS output: a chain
/// rooted at an imported binding (the CommonJS transform rewrites its root
/// to `m_1.a`), a call chain rooted at an imported function (`(0,
/// m_1.f)()...`), a chain of constant-enum members (the inliner replaces
/// each with its value) and a chain on an ambient value, which the inliner
/// asks the checker's `GetConstantValue` about at every link (the checker
/// resolves the chain's entity name once per link). Each is emitted
/// single-threaded on a 256 KiB thread and concurrently on the work group's
/// reserved stacks with the same output, and the whole depth is rewritten.
#[test]
fn deep_chains_through_the_module_and_inliner_transforms() {
    let n = DEPTH;
    let module = (
        "/m.ts",
        "export const a: any = 1;\nexport function f(): any { return f; }\n",
    );
    let emit = |text: String| {
        support::emit_deep(
            &files(&[("/lib.d.ts", LIB), module, ("/x.ts", &text)]),
            &commonjs(),
        )
        .text("/x.js")
        .to_vec()
    };
    let imported = emit(format!(
        "import {{ a }} from \"./m\";\nexport const x: number = a{};\n",
        ".b".repeat(n)
    ));
    assert_eq!(support::occurrences(&imported, "m_1.a.b"), 1);
    assert_eq!(support::occurrences(&imported, ".b"), n);
    let calls = emit(format!(
        "import {{ f }} from \"./m\";\nexport const y: number = f(){};\n",
        "()".repeat(n - 1)
    ));
    assert_eq!(support::occurrences(&calls, "(0, m_1.f)()"), 1);
    assert_eq!(support::occurrences(&calls, "()"), n);
    let inlined = emit(format!(
        "const enum E {{ A = 1 }}\nexport const z = {};\n",
        support::repeat("E.A", n, " + ")
    ));
    assert_eq!(support::occurrences(&inlined, "1 /* E.A */"), n);
    let ambient = emit(format!(
        "declare const o: any;\nexport const w: number = o{};\n",
        ".b".repeat(n)
    ));
    assert_eq!(support::occurrences(&ambient, "exports.w = o.b"), 1);
    assert_eq!(support::occurrences(&ambient, ".b"), n);
}
