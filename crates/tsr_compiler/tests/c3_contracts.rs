//! Phase 2 C3 direct contracts (docs/PHASE2-C3-plan.md, C3.8), over production
//! entry points. Contracts 2 to 5 and the C3.6 and C3.7 cases live in
//! `support/c3_changes.rs` as pinned native-observed fixtures.
#[path = "support/c3_changes.rs"]
mod changes;
#[path = "support/c3_native_diagnostics.rs"]
mod native;
#[path = "support/c2_contract_program.rs"]
mod support;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::ops::ControlFlow;
use std::sync::Arc;
use tsr_arena::NodeId;
use tsr_ast::{AstView, ChildVisitor, SyntaxKind as K};
use tsr_checker::CheckerOwner;
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
/// command line stops at the syntax errors, so the checker's own diagnostics
/// come from a pinned Go oracle (`fixtures/c3/malformed/`): the parser's, the
/// binder's, the checker's and the global sets each match natively, the same
/// checker answers a later query, and the sets repeat unchanged.
#[test]
fn malformed_input_checks_to_completion_and_keeps_answering() {
    let requests: Value =
        serde_json::from_str(include_str!("fixtures/c3/malformed/requests.json")).unwrap();
    let native: Value =
        serde_json::from_str(include_str!("fixtures/c3/malformed/native.json")).unwrap();
    let provenance: Value =
        serde_json::from_str(include_str!("fixtures/c3/malformed/provenance.json")).unwrap();
    let pin: Value = serde_json::from_str(include_str!("../../../data/upstream.json")).unwrap();
    let digest = |bytes: &[u8]| json!(format!("{:x}", Sha256::digest(bytes)));
    assert_eq!(provenance["pin"], pin["pin"]);
    assert_eq!(native["request_sha256"], provenance["request_sha256"]);
    assert_eq!(
        provenance["request_sha256"],
        digest(include_bytes!("fixtures/c3/malformed/requests.json"))
    );
    assert_eq!(
        provenance["source_sha256"],
        digest(include_bytes!("fixtures/c3/malformed.ts"))
    );
    let source = requests[0]["files"]["/main.ts"].as_str().unwrap();
    assert_eq!(source, include_str!("fixtures/c3/malformed.ts"));
    let program = support::program(&[("/main.ts", source)]);
    let (_, _, owner) = support::checker(&program);
    let file = program.file(b"/main.ts").unwrap();
    let render = |diagnostics: &[tsr_ast::Diagnostic]| {
        diagnostics
            .iter()
            .map(|d| native::diagnostic_json(&program, d))
            .collect::<Vec<_>>()
    };
    let observe = |owner: &Arc<CheckerOwner>| {
        let mut op = owner.operation().unwrap();
        json!({
            "syntactic": render(&program.syntactic_diagnostics(Some(file)).unwrap()),
            "bind": render(&program.bind_diagnostics(Some(file.source())).unwrap()),
            "checker": render(&op.semantic_diagnostics(file.source()).unwrap()),
            "global": render(&op.global_diagnostics().unwrap()),
        })
    };
    let row = &native["rows"][0];
    let expected = json!({
        "syntactic": row["syntactic"], "bind": row["bind"], "checker": row["checker"], "global": row["global"],
    });
    assert!(!row["syntactic"].as_array().unwrap().is_empty());
    assert!(!row["checker"].as_array().unwrap().is_empty());
    assert_eq!(observe(&owner), expected);
    let op = owner.operation().unwrap();
    assert!(op.builtin_type("stringType").is_some());
    drop(op);
    assert_eq!(observe(&owner), expected);
}

/// Contract 9: a deep expression chain (the corpus row
/// binderBinaryExpressionStress, thousands of left-nested operators) checks on
/// the E2 small stack and gives the same diagnostics as an ordinary stack. The
/// observer on the expression path shows the recursion reaching one level per
/// operator and running on a grown stack segment larger than the thread's.
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
    let operators = SOURCE
        .lines()
        .map(|line| line.matches(" + ").count())
        .max()
        .unwrap();
    assert!(
        operators > 1000,
        "{operators} operators on the longest line"
    );
    let depth = observed.1["maximum_expression_depth"].as_u64().unwrap();
    assert!(
        depth >= operators as u64,
        "expression depth {depth} for {operators} operators: {}",
        observed.1
    );
    let remaining = observed.1["maximum_remaining_stack"].as_u64().unwrap();
    assert!(
        remaining > STACK as u64,
        "no stack segment beyond the {STACK}-byte thread stack: {}",
        observed.1
    );
}
