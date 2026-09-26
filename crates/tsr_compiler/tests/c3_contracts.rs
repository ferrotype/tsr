//! Phase 2 C3 direct contracts (docs/PHASE2-C3-plan.md, C3.8), over production
//! entry points. Contracts 2 to 5 and the C3.6 and C3.7 cases live in
//! `support/c3_changes.rs` as pinned native-observed fixtures.
#[path = "support/c3_changes.rs"]
mod changes;
#[path = "support/c3_native_diagnostics.rs"]
mod native;
#[path = "support/c2_contract_program.rs"]
mod support;
use serde_json::json;
use std::collections::HashMap;
use std::ops::ControlFlow;
use tsr_arena::NodeId;
use tsr_ast::{AstView, ChildVisitor, SyntaxKind as K};
use tsr_compiler::Program;

const NARROWING: &str = r#"
type Shape = { kind: "circle"; r: number } | { kind: "square"; s: number };
export function area(shape: Shape | undefined) {
  if (shape?.kind === "circle") { return shape.r; }
  const items = [];
  items.push(1);
  return shape ? shape.s : items.length;
}
"#;

fn diagnostics(
    program: &std::sync::Arc<Program>,
    owner: &std::sync::Arc<tsr_checker::CheckerOwner>,
) -> Vec<(i32, i64)> {
    let file = program.file(b"/main.ts").unwrap();
    let mut op = owner.operation().unwrap();
    op.semantic_diagnostics(file.source())
        .unwrap()
        .iter()
        .map(|d| (d.code, d.loc.pos()))
        .collect()
}

/// Contract 1: two checkers over one program narrow the same reference
/// independently, and retiring one generation leaves the other answering
/// (`Checker.getFlowTypeOfReference` state is checker-local in the pin).
#[test]
fn two_checkers_narrow_independently_and_retirement_is_isolated() {
    let program = support::program(&[("/main.ts", NARROWING)]);
    let (_, generation_a, owner_a) = support::checker(&program);
    let (_, _generation_b, owner_b) = support::checker(&program);
    let first = diagnostics(&program, &owner_a);
    let second = diagnostics(&program, &owner_b);
    assert_eq!(first, second);
    assert!(first.is_empty(), "{first:?}");
    generation_a.retire();
    assert!(
        owner_a.operation().is_err(),
        "a retired checker must refuse a new operation"
    );
    assert_eq!(diagnostics(&program, &owner_b), second);
}

fn import_specifiers(program: &Program, file: &str) -> HashMap<String, NodeId> {
    struct Walk<'a> {
        view: AstView<'a>,
        nodes: HashMap<String, NodeId>,
    }
    impl ChildVisitor for Walk<'_> {
        fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
            let read = self.view.node(node).unwrap();
            if matches!(
                read.kind().known(),
                Some(K::ImportSpecifier | K::ExportSpecifier)
            ) {
                if let Some(name) = read.name() {
                    let text =
                        String::from_utf8(self.view.node_text(name).unwrap().as_bytes().to_vec())
                            .unwrap();
                    let key = if read.kind() == K::ExportSpecifier {
                        format!("export {text}")
                    } else {
                        text
                    };
                    self.nodes.insert(key, node);
                }
            }
            read.for_each_child(self)
        }
        fn visit_list(&mut self, list: tsr_ast::NodeListId) -> ControlFlow<()> {
            self.visit_node_slice(self.view.list(list).unwrap().nodes())
        }
        fn visit_node_slice(&mut self, slice: tsr_ast::NodeSlice) -> ControlFlow<()> {
            for node in self.view.node_slice(slice).unwrap().iter().flatten() {
                self.visit_node(node)?;
            }
            ControlFlow::Continue(())
        }
    }
    let file = program.file(file.as_bytes()).unwrap();
    let mut walk = Walk {
        view: file.bound().view().ast(),
        nodes: HashMap::new(),
    };
    let _ = walk.visit_node(file.source());
    walk.nodes
}

/// Contract 6: alias state as the pin's `markAliasReferenced` and
/// `markSymbolOfAliasDeclarationIfTypeOnly` leave it, observed through the
/// checker's alias links (the resolver half is C5.6's).
#[test]
fn alias_links_record_referenced_and_type_only_state() {
    let program = support::program(&[
        ("/m.ts", "export type T = number; export const v = 1; export class U { a = 1; }"),
        ("/main.ts", "import { T, v, U } from \"./m\";\nimport type { U as U2 } from \"./m\";\nexport { v };\nlet t: T = 1;\nlet u: U2 | undefined;\nlet w = v + 1;\n"),
    ]);
    let (_, _, owner) = support::checker(&program);
    let file = program.file(b"/main.ts").unwrap();
    let specifiers = import_specifiers(&program, "/main.ts");
    let mut op = owner.operation().unwrap();
    let errors = op.semantic_diagnostics(file.source()).unwrap();
    assert!(
        errors.is_empty(),
        "{:?}",
        errors.iter().map(|d| d.code).collect::<Vec<_>>()
    );
    let state = |op: &mut tsr_checker::Operation<'_>, name: &str| {
        op.alias_link_state(specifiers[name]).unwrap()
    };
    assert_eq!(
        state(&mut op, "v"),
        json!({"referenced": true, "type_only": false})
    );
    assert_eq!(
        state(&mut op, "T"),
        json!({"referenced": false, "type_only": false})
    );
    assert_eq!(
        state(&mut op, "U2"),
        json!({"referenced": false, "type_only": true})
    );
    assert_eq!(state(&mut op, "U")["referenced"], json!(false));
}

/// Contract 8: a file with parse errors is checked to completion. The pinned
/// command line reports only the syntactic diagnostics of such a file, so the
/// fixture binds those; the semantic pass must still complete without a
/// harness error, and the checker answers later queries and repeats itself.
#[test]
fn malformed_input_checks_to_completion_and_keeps_answering() {
    let fixture = native::load(
        "malformed.ts",
        include_str!("fixtures/c3/malformed.ts"),
        include_str!("fixtures/c3/malformed.ts.native.json"),
    );
    let first = native::observed(&fixture);
    let syntactic = fixture.native["diagnostics"].as_array().unwrap();
    assert_eq!(&first[..syntactic.len()], syntactic.as_slice());
    assert!(
        first.len() > syntactic.len(),
        "the semantic pass reported nothing"
    );
    let op = fixture.owner.operation().unwrap();
    assert!(op.builtin_type("stringType").is_some());
    drop(op);
    assert_eq!(native::observed(&fixture), first);
}

/// Contract 9: a deep flow graph (the corpus row binderBinaryExpressionStress)
/// checks on the E2 small stack with the recursion observer engaged and gives
/// the same diagnostics as an ordinary stack.
#[test]
fn deep_flow_graph_checks_on_a_small_stack() {
    const STACK: usize = 256 * 1024;
    const SOURCE: &str = include_str!(
        "../../../upstream/tsc/testdata/tests/cases/compiler/binderBinaryExpressionStress.ts"
    );
    let reference = {
        let program = support::program(&[("/main.ts", SOURCE)]);
        let (_, _, owner) = support::checker(&program);
        diagnostics(&program, &owner)
    };
    let observed = std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(move || {
            let program = support::program(&[("/main.ts", SOURCE)]);
            let (_, _, owner) = support::checker(&program);
            let file = program.file(b"/main.ts").unwrap();
            let mut op = owner.operation().unwrap();
            op.begin_recursion_probe(None).unwrap();
            let codes: Vec<(i32, i64)> = op
                .semantic_diagnostics(file.source())
                .unwrap()
                .iter()
                .map(|d| (d.code, d.loc.pos()))
                .collect();
            let probe = op.take_recursion_probe().unwrap();
            (codes, probe)
        })
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(observed.0, reference);
    assert!(
        observed.1["maximum_depth"].as_u64().is_some(),
        "{}",
        observed.1
    );
}
