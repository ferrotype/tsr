//! C2 measured variance flags and production complexity-limit contracts. Native
//! schedules and immutable observations live beside their source requests.
#![cfg(feature = "recursion-probe")]
use super::{diagnostic, support};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::ops::ControlFlow;
use tsr_arena::NodeId;
use tsr_ast::{AstView, ChildVisitor, SyntaxKind as K};
use tsr_compiler::Program;

const REQUESTS: &str = include_str!("../fixtures/c2/variance_limits/requests.json");
const NATIVE: &str = include_str!("../fixtures/c2/variance_limits/native.json");
const PROVENANCE: &str = include_str!("../fixtures/c2/variance_limits/provenance.json");

fn fixtures(id: &str) -> (Value, Value) {
    let requests: Value = serde_json::from_str(REQUESTS).unwrap();
    let native: Value = serde_json::from_str(NATIVE).unwrap();
    let provenance: Value = serde_json::from_str(PROVENANCE).unwrap();
    let pin: Value = serde_json::from_str(include_str!("../../../../data/upstream.json")).unwrap();
    let hash = |b: &[u8]| format!("{:x}", Sha256::digest(b));
    assert_eq!(provenance["pin"], pin["pin"]);
    assert_eq!(provenance["request_sha256"], hash(REQUESTS.as_bytes()));
    assert_eq!(native["request_sha256"], provenance["request_sha256"]);
    assert_eq!(provenance["output_sha256"], hash(NATIVE.as_bytes()));
    for (name, bytes) in [
        (
            "oracle_bridge.go",
            include_bytes!("../fixtures/c2/variance_limits/oracle_bridge.go").as_slice(),
        ),
        (
            "oracle_test.go",
            include_bytes!("../fixtures/c2/variance_limits/oracle_test.go").as_slice(),
        ),
        (
            "regenerate.py",
            include_bytes!("../fixtures/c2/variance_limits/regenerate.py").as_slice(),
        ),
    ] {
        assert_eq!(provenance["observer_sources"][name], hash(bytes));
    }
    let row = |rows: &Value| {
        rows.as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == id)
            .unwrap()
            .clone()
    };
    (row(&requests), row(&native["rows"]))
}

fn declarations(program: &Program) -> HashMap<String, NodeId> {
    struct Walk<'a> {
        view: AstView<'a>,
        nodes: HashMap<String, NodeId>,
    }
    impl ChildVisitor for Walk<'_> {
        fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
            let read = self.view.node(node).unwrap();
            if matches!(
                read.kind().known(),
                Some(
                    K::VariableDeclaration
                        | K::FunctionDeclaration
                        | K::TypeAliasDeclaration
                        | K::InterfaceDeclaration
                        | K::ClassDeclaration
                        | K::Parameter
                        | K::TypeParameter
                )
            ) {
                if let Some(name) = read.name() {
                    self.nodes.insert(
                        String::from_utf8(self.view.node_text(name).unwrap().as_bytes().to_vec())
                            .unwrap(),
                        node,
                    );
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
    let file = program.file(b"/main.ts").unwrap();
    let mut walk = Walk {
        view: file.bound().view().ast(),
        nodes: HashMap::new(),
    };
    let _ = walk.visit_node(file.source());
    walk.nodes
}

fn observe(id: &str) {
    let (request, expected) = fixtures(id);
    let program = support::program(&[("/main.ts", request["source"].as_str().unwrap())]);
    let nodes = declarations(&program);
    let file = program.file(b"/main.ts").unwrap();
    let view = file.bound().view().ast();
    let (_counters, _generation, owner) = support::checker(&program);
    let mut op = owner.operation().unwrap();
    let name = |key: &str| view.node(nodes[key]).unwrap().name().unwrap();
    if request["mode"] == "variance" {
        let mut measurements = Vec::new();
        for query in request["queries"].as_array().unwrap() {
            measurements.push(
                op.c2_variance_contract(name(query.as_str().unwrap()))
                    .unwrap(),
            );
        }
        assert_eq!(
            json!(measurements),
            expected["measurements"],
            "{id} actual variance measurements"
        );
    } else {
        op.c2_begin_limits_probe();
        let target = name(request["target"].as_str().unwrap());
        let result = match request["mode"].as_str().unwrap() {
            "count" => op.c2_instantiation_count_contract(target).unwrap(),
            "subtypes" => op.c2_subtype_limit_contract(target).unwrap(),
            "constraint" => op.c2_base_constraint_contract(target).unwrap(),
            "nesting" => json!(request["stacks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|stack| {
                    let stack = stack
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|s| name(s.as_str().unwrap()))
                        .collect::<Vec<_>>();
                    op.c2_nested_contract(target, &stack, request["max"].as_u64().unwrap() as usize)
                        .unwrap()
                })
                .collect::<Vec<_>>()),
            mode => panic!("unknown mode {mode}"),
        };
        assert_eq!(result, expected["result"], "{id} result");
        assert_eq!(
            op.c2_take_limits_probe(),
            expected["limits"],
            "{id} actual guard hits"
        );
        let recovery = op.get_type_at_location(name("recovery")).unwrap();
        let display = op.type_to_string(recovery, 0).unwrap();
        assert_eq!(
            String::from_utf8(display.as_bytes().to_vec()).unwrap(),
            expected["recovery"].as_str().unwrap()
        );
        assert_eq!(
            json!(op.c2_instantiation_count()),
            expected["recovery_count"]
        );
    }
    let diagnostics = op.semantic_diagnostics(file.source()).unwrap();
    assert_eq!(
        json!(diagnostics
            .iter()
            .map(|d| diagnostic(&program, d))
            .collect::<Vec<_>>()),
        expected["diagnostics"],
        "{id} diagnostics"
    );
}

/// Go getVariancesWorker: invariant, co/contra/bi/independent and reliability bits.
#[test]
fn all_seven_variance_flags_are_measured() {
    observe("variance_flags");
}

/// Go getVariancesWorker: empty cycle result, smallest-symbol restart, cache reuse.
#[test]
fn recursive_variance_is_stable_across_entry_order() {
    observe("cycle_forward");
    observe("cycle_reverse");
}

/// Go instantiateTypeWithAlias called repeatedly with a real mapper. Count is
/// never seeded; the following GetTypeAtLocation performs its ordinary reset.
#[test]
fn actual_five_million_instantiation_guard_recovers() {
    observe("count_limit");
}

/// Go getUnionTypeEx(Subtype) -> removeSubtypes counts 100,000 real candidate pairs.
#[test]
fn actual_subtype_work_estimate_matches_native() {
    observe("subtype_1000");
    observe("subtype_1020");
}

/// Go getBaseConstraintOfType -> getResolvedBaseConstraint / computeBaseConstraint.
#[test]
fn actual_base_and_conditional_constraint_limits_match_native() {
    observe("base_constraint_49");
    observe("base_constraint_51");
    observe("conditional_constraint_101");
}

/// Go isDeeplyNestedType over source-resolved conditional and mapped roots.
#[test]
fn deeply_nested_conditional_and_mapped_roots_match_native() {
    observe("conditional_roots");
    observe("mapped_roots");
}
