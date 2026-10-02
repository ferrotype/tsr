//! Phase 3 T5 contracts (docs/PHASE3-plan.md, sections 4 and 5): the
//! downlevel transforms' helpers belong to the file that needs them, and the
//! downlevel transforms carry deep inputs through their growth guards.
#[allow(dead_code)]
#[path = "support/phase3_contracts.rs"]
mod support;

use support::{files, Mode, LIB};
use tsr_core::{CompilerOptions, ModuleKind, ScriptTarget};

fn target(target: ScriptTarget) -> CompilerOptions {
    CompilerOptions {
        target,
        module: ModuleKind::ESNEXT,
        ..CompilerOptions::default()
    }
}

const PLAIN: &str =
    "export const plain: number = 3;\nexport function twice(p: number): number { return p * 2; }\n";
const DOWNLEVEL: &str = "declare const source: any;\n\
export async function load(): Promise<number> { return await source; }\n\
export const { first, ...rest } = source;\n\
export const merged = { ...source, extra: 1 };\n\
export class Counter { #count = 0; static #instances = 0; increment(): number { Counter.#instances++; return ++this.#count; } }\n\
export const tagged = String.raw`\\u{${source}`;\n";

/// The helpers the downlevel transforms request at ES2015 (`__awaiter` for
/// `async`, `__rest` for an object rest, `__classPrivateField*` for private
/// fields, `__makeTemplateObject` for a tagged template with an invalid
/// escape) are recorded in the emit context of the file that needs them and
/// printed only there: in a program whose middle file needs them (the
/// single-threaded group emits the last file first, so a file emitted after
/// it precedes it), the other files print exactly what they print in a
/// program without that file, in both modes.
#[test]
fn downlevel_helpers_belong_to_the_file_that_needs_them() {
    let helpers = [
        "__awaiter",
        "__rest",
        "__classPrivateFieldGet",
        "__classPrivateFieldSet",
        "__makeTemplateObject",
    ];
    let alone = files(&[("/lib.d.ts", LIB), ("/a.ts", PLAIN), ("/c.ts", PLAIN)]);
    let with_downlevel = files(&[
        ("/lib.d.ts", LIB),
        ("/a.ts", PLAIN),
        ("/b.ts", DOWNLEVEL),
        ("/c.ts", PLAIN),
    ]);
    let options = target(ScriptTarget::ES2015);
    for mode in Mode::BOTH {
        let (checked, _) = support::checked(&alone, &options, mode);
        let expected = support::emit_all(&checked);
        let (checked, _) = support::checked(&with_downlevel, &options, mode);
        let observed = support::emit_all(&checked);
        let downlevel = String::from_utf8_lossy(observed.text("/b.js")).into_owned();
        for helper in helpers {
            assert!(downlevel.contains(helper), "{mode:?}: /b.js lacks {helper}");
        }
        for name in ["/a.js", "/c.js"] {
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

/// Depth of the deep inputs through the downlevel transforms.
const DEPTH: usize = 500;

/// Deep inputs through the downlevel transforms (ADR 0011): `DEPTH` links of
/// an optional chain at ES2019 (the optional-chain transform visits a
/// chain's left side directly, once per link), a left-nested `??` chain at
/// ES2019, a right-associative `**` chain at ES2015, a right-nested `||=`
/// chain at ES2020 and nested async arrow functions at ES2016, and a tenth
/// of that of nested classes with fields at ES2015 (the class-fields
/// transform walks each member body for constructor references, so the walk
/// is quadratic). Each is emitted single-threaded on a 256 KiB thread and
/// concurrently on the work group's reserved stacks with the same output,
/// and the whole depth is lowered.
#[test]
fn deep_inputs_downlevel_through_the_growth_guards() {
    let n = DEPTH;
    let emit = |text: String, options: &CompilerOptions| {
        support::emit_deep(&files(&[("/lib.d.ts", LIB), ("/a.ts", &text)]), options)
            .text("/a.js")
            .to_vec()
    };
    let optional = emit(
        format!(
            "declare const a: any;\nexport const x: number = a{};\n",
            "?.b".repeat(n)
        ),
        &target(ScriptTarget::ES2019),
    );
    assert_eq!(support::occurrences(&optional, "?."), 0);
    assert_eq!(support::occurrences(&optional, "void 0 : "), n);
    let nullish = emit(
        format!(
            "declare const a: any;\nexport const x: number = {};\n",
            support::repeat("a", n, " ?? ")
        ),
        &target(ScriptTarget::ES2019),
    );
    assert_eq!(support::occurrences(&nullish, "??"), 0);
    assert_eq!(support::occurrences(&nullish, "!== null"), n - 1);
    let power = emit(
        format!(
            "declare const a: number;\nexport const x: number = {};\n",
            support::repeat("a", n, " ** ")
        ),
        &target(ScriptTarget::ES2015),
    );
    assert_eq!(support::occurrences(&power, "**"), 0);
    assert_eq!(support::occurrences(&power, "Math.pow("), n - 1);
    let assignments = emit(
        format!(
            "declare let a: any;\nexport function f(): void {{ {} = 1; }}\n",
            support::repeat("a", n, " ||= ")
        ),
        &target(ScriptTarget::ES2020),
    );
    assert_eq!(support::occurrences(&assignments, "||="), 0);
    assert_eq!(support::occurrences(&assignments, "a || (a = "), n - 1);
    let arrows = emit(
        format!(
            "export const f = {}1{};\n",
            "async () => await (".repeat(n),
            ")".repeat(n)
        ),
        &target(ScriptTarget::ES2016),
    );
    assert_eq!(support::occurrences(&arrows, "async"), 0);
    assert_eq!(support::occurrences(&arrows, "=> __awaiter("), n);
    let class_depth = n / 10;
    let fields = emit(
        format!(
            "export function g(): void {{ {}{} }}\n",
            support::numbered(class_depth, |i| format!(
                "class C{i} {{ x = 1; static y = 2; #p = 3; m(): void {{ "
            )),
            "} } ".repeat(class_depth)
        ),
        &target(ScriptTarget::ES2015),
    );
    assert_eq!(support::occurrences(&fields, "#p"), 0);
    assert_eq!(
        support::occurrences(&fields, "= new WeakMap();"),
        class_depth
    );
}

/// Depth of the nested `using` blocks: the transform's cost grows about as
/// the cube of the depth (300 blocks take over five minutes in a debug
/// build), and without its growth guard 70 blocks overflow the 256 KiB
/// thread.
const USING_DEPTH: usize = 100;

/// `USING_DEPTH` nested blocks with a `using` declaration each, at ES2022:
/// the `using` transform visits a block's statements directly, once per
/// nested block (`usingDeclarationTransformer.visit`). Emitted
/// single-threaded on a 256 KiB thread and concurrently on the work group's
/// reserved stacks with the same output; every block gets its disposable
/// resource.
#[test]
fn deep_using_blocks_downlevel_through_the_growth_guards() {
    let n = USING_DEPTH;
    let text = format!(
        "export function f(): void {{ {}{} }}\n",
        "{ using a = null; ".repeat(n),
        "}".repeat(n)
    );
    let observed = support::emit_deep(
        &files(&[("/lib.d.ts", LIB), ("/a.ts", &text)]),
        &target(ScriptTarget::ES2022),
    );
    assert_eq!(
        support::occurrences(observed.text("/a.js"), "= __addDisposableResource(env_"),
        n
    );
}
