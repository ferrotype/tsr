//! Repeated emits must not retain identities from each fresh transform arena.
#[allow(dead_code)]
#[path = "support/phase3_contracts.rs"]
mod support;

use support::{files, Mode, LIB};
use tsr_core::{CompilerOptions, JsxEmit, ModuleKind, ScriptTarget, Tristate};

#[test]
fn repeated_jsx_and_metadata_emits_reuse_reference_nodes_and_release_outputs() {
    let mut jsx = String::from("declare const React: any;\n");
    for i in 0..100 {
        jsx.push_str(&format!("export const x{i} = <div/>;\n"));
    }
    let mut metadata =
        String::from("import { Value } from './value';\nfunction dec(...args: any[]): void {}\n");
    for i in 0..30 {
        metadata.push_str(&format!(
            "export class C{i} {{ @dec method(value: Value): void {{}} }}\n"
        ));
    }
    let cases = [
        ("/a.tsx", &jsx, JsxEmit::REACT, false),
        ("/a.tsx", &jsx, JsxEmit::REACT_JSX, false),
        ("/a.ts", &metadata, JsxEmit::NONE, true),
    ];
    for (name, source, jsx, decorated) in cases {
        let inputs = files(&[
            ("/lib.d.ts", LIB),
            ("/value.ts", "export class Value {}\n"),
            (name, source),
        ]);
        let options = CompilerOptions {
            target: ScriptTarget::ES2017,
            module: ModuleKind::COMMON_JS,
            jsx,
            no_check: Tristate::TRUE,
            experimental_decorators: Tristate::from(decorated),
            emit_decorator_metadata: Tristate::from(decorated),
            ..CompilerOptions::default()
        };
        for mode in Mode::BOTH {
            let (checked, counters) = support::checked(&inputs, &options, mode);
            let expected = support::emit_all(&checked);
            let after_first = counters.snapshot();
            for _ in 0..20 {
                assert_eq!(support::emit_all(&checked), expected);
                assert_eq!(counters.snapshot(), after_first, "{jsx:?}, {mode:?}");
            }
            drop(checked);
            // Keeping the emitted bytes alive must keep no AST/checker owner.
            assert_eq!(counters.snapshot(), tsr_arena::Counts::default());
            assert!(!expected.text("/a.js").is_empty());
        }
    }
}
