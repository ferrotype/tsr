//! Phase 3 T6 contracts (docs/PHASE3-plan.md, sections 4 and 5): the JSX
//! transform's runtime imports belong to the file that uses JSX, and every
//! JSX mode carries deep nesting through its growth guards.
#[allow(dead_code)]
#[path = "support/phase3_contracts.rs"]
mod support;

use support::{files, Mode, LIB};
use tsr_core::{CompilerOptions, JsxEmit, ModuleKind, ScriptTarget};

const JSX_TYPES: &str =
    "declare namespace JSX { interface IntrinsicElements { [name: string]: any } }\n";

fn jsx(mode: JsxEmit) -> CompilerOptions {
    CompilerOptions {
        target: ScriptTarget::ESNEXT,
        module: ModuleKind::ESNEXT,
        jsx: mode,
        ..CompilerOptions::default()
    }
}

/// The automatic runtime's imports (`react/jsx-runtime`'s `jsx`, `jsxs` and
/// `Fragment` under `react-jsx`, `react/jsx-dev-runtime`'s `jsxDEV` and
/// `Fragment` under `react-jsxdev`, and `react`'s `createElement` for an
/// element with a spread before a `key`) are collected by the JSX transform
/// of the file that uses them and printed only there: in a program of four
/// `.tsx` files that each need a different set, one of them none, each file
/// prints exactly what it prints when it is the program's only file, in
/// both modes.
#[test]
fn jsx_runtime_imports_belong_to_the_files_that_use_them() {
    let sources = [
        ("/a.tsx", "export const single = <div>{\"text\"}</div>;\n"),
        (
            "/b.tsx",
            "export const fragment = <><div /><span /></>;\n",
        ),
        (
            "/c.tsx",
            "declare const props: any;\nexport const spread = <div {...props} key=\"k\" />;\n",
        ),
        (
            "/d.tsx",
            "export const plain: number = 1;\nexport function twice(p: number): number { return p * 2; }\n",
        ),
    ];
    for emit in [JsxEmit::REACT_JSX, JsxEmit::REACT_JSX_DEV] {
        for mode in Mode::BOTH {
            let mut mixed = files(&[("/lib.d.ts", LIB), ("/jsx.d.ts", JSX_TYPES)]);
            mixed.extend(files(&sources));
            let (checked, _) = support::checked(&mixed, &jsx(emit), mode);
            let observed = support::emit_all(&checked);
            for (name, text) in sources {
                let alone = files(&[("/lib.d.ts", LIB), ("/jsx.d.ts", JSX_TYPES), (name, text)]);
                let (checked, _) = support::checked(&alone, &jsx(emit), mode);
                let output = name.replace(".tsx", ".js");
                assert_eq!(
                    String::from_utf8_lossy(observed.text(&output)),
                    String::from_utf8_lossy(support::emit_all(&checked).text(&output)),
                    "{emit:?} {mode:?}: {output}"
                );
            }
            let fragment = String::from_utf8_lossy(observed.text("/b.js")).into_owned();
            assert!(fragment.contains("Fragment as _Fragment"), "{fragment}");
            let spread = String::from_utf8_lossy(observed.text("/c.js")).into_owned();
            assert!(
                spread.contains("createElement as _createElement"),
                "{spread}"
            );
            assert_eq!(support::occurrences(observed.text("/a.js"), "Fragment"), 0);
            assert_eq!(
                support::occurrences(observed.text("/a.js"), "createElement"),
                0
            );
            assert_eq!(support::occurrences(observed.text("/d.js"), "react"), 0);
        }
    }
}

/// Depth of the nested elements.
const DEPTH: usize = 500;

/// `DEPTH` nested elements under every JSX mode (ADR 0011): printed as JSX
/// under `preserve` and `react-native` (the printer emits a JSX child
/// directly, once per level), and transformed under `react`, `react-jsx` and
/// `react-jsxdev` (the transform visits each child directly, once per
/// level). Each is emitted single-threaded on a 256 KiB thread and
/// concurrently on the work group's reserved stacks with the same output,
/// and every level is in it.
#[test]
fn deep_jsx_through_the_growth_guards_in_every_mode() {
    let n = DEPTH;
    let text = format!(
        "{JSX_TYPES}declare var React: any;\nexport const x = {}{};\n",
        "<a>".repeat(n),
        "</a>".repeat(n)
    );
    for (emit, output, element) in [
        (JsxEmit::PRESERVE, "/a.jsx", "<a>"),
        (JsxEmit::REACT_NATIVE, "/a.js", "<a>"),
        (JsxEmit::REACT, "/a.js", "React.createElement(\"a\""),
        (JsxEmit::REACT_JSX, "/a.js", "_jsx(\"a\""),
        (JsxEmit::REACT_JSX_DEV, "/a.js", "_jsxDEV(\"a\""),
    ] {
        let observed =
            support::emit_deep(&files(&[("/lib.d.ts", LIB), ("/a.tsx", &text)]), &jsx(emit));
        assert_eq!(
            support::occurrences(observed.text(output), element),
            n,
            "{element}"
        );
    }
}
