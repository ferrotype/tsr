//! C2.11 direct production contracts. The native overlay schedule and immutable
//! observations live beside their source requests; no expectation comes from Rust.
#![cfg(feature = "recursion-probe")]
#[path = "support/c2_contract_program.rs"]
mod support;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::ops::ControlFlow;
use tsr_arena::NodeId;
use tsr_ast::{AstView, ChildVisitor, SyntaxKind as K};
use tsr_compiler::Program;

const REQUESTS: &str = include_str!("fixtures/c2/contracts/requests.json");
const NATIVE: &str = include_str!("fixtures/c2/contracts/native.json");
const PROVENANCE: &str = include_str!("fixtures/c2/contracts/provenance.json");

fn fixtures(id: &str) -> (Value, Value) {
    let requests: Value = serde_json::from_str(REQUESTS).unwrap();
    let native: Value = serde_json::from_str(NATIVE).unwrap();
    let provenance: Value = serde_json::from_str(PROVENANCE).unwrap();
    let pin: Value = serde_json::from_str(include_str!("../../../data/upstream.json")).unwrap();
    let hash = |b: &[u8]| format!("{:x}", Sha256::digest(b));
    assert_eq!(provenance["pin"], pin["pin"]);
    assert_eq!(provenance["request_sha256"], hash(REQUESTS.as_bytes()));
    assert_eq!(native["request_sha256"], provenance["request_sha256"]);
    assert_eq!(provenance["output_sha256"], hash(NATIVE.as_bytes()));
    for (name, bytes) in [
        (
            "oracle_bridge.go",
            include_bytes!("fixtures/c2/contracts/oracle_bridge.go").as_slice(),
        ),
        (
            "oracle_test.go",
            include_bytes!("fixtures/c2/contracts/oracle_test.go").as_slice(),
        ),
        (
            "regenerate.py",
            include_bytes!("fixtures/c2/contracts/regenerate.py").as_slice(),
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
                Some(K::VariableDeclaration | K::FunctionDeclaration | K::TypeAliasDeclaration)
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

fn diagnostic(program: &Program, d: &tsr_ast::Diagnostic) -> Value {
    let file = d.file.map(|id| {
        let file = program
            .files()
            .iter()
            .find(|file| file.source() == id)
            .expect("diagnostic source retained");
        let name = file.bound().view().source_file().unwrap().file_name();
        String::from_utf8(name.rsplit(|&c| c == b'/').next().unwrap().to_vec()).unwrap()
    });
    json!({"file":file,"pos":d.loc.pos(),"end":d.loc.end(),"code":d.code,"category":d.category,
        "message":String::from_utf8(tsr_compiler::diagnostic_writer::localized(d).unwrap()).unwrap(),
        "chain":d.message_chain.iter().map(|d|diagnostic(program,d)).collect::<Vec<_>>(),
        "related":d.related_information.iter().map(|d|diagnostic(program,d)).collect::<Vec<_>>()})
}

fn observe(id: &str, small_stack: Option<usize>) {
    let (request, expected) = fixtures(id);
    let program = support::program(&[("/main.ts", request["source"].as_str().unwrap())]);
    let nodes = declarations(&program);
    let file = program.file(b"/main.ts").unwrap();
    let view = file.bound().view().ast();
    let (_counters, _generation, owner) = support::checker(&program);
    let mut op = owner.operation().unwrap();
    if request["mode"] == "inference" {
        assert_eq!(
            op.c2_inference_contract(view.node(nodes["seed"]).unwrap().name().unwrap())
                .unwrap(),
            expected["inference"]
        );
    }
    assert_eq!(
        json!(op.c2_context_depth()),
        expected["before_context_depth"]
    );
    op.c2_begin_instantiation_probe().unwrap();
    let diagnostics = op.semantic_diagnostics(file.source()).unwrap();
    assert_eq!(
        json!(diagnostics
            .iter()
            .map(|d| diagnostic(&program, d))
            .collect::<Vec<_>>()),
        expected["diagnostics"],
        "{id} diagnostics"
    );
    assert_eq!(
        json!(op.c2_context_depth()),
        expected["after_context_depth"]
    );
    let mut instantiated = op.c2_take_instantiation_probe().unwrap();
    if let Some(stack) = small_stack {
        assert!(
            instantiated["maximum_remaining_stack"].as_u64().unwrap() > stack as u64,
            "{instantiated}"
        );
        assert_eq!(instantiated["maximum_depth"], 100);
        assert!(instantiated["depth_limit_hits"].as_u64().unwrap() > 0);
        instantiated
            .as_object_mut()
            .unwrap()
            .remove("maximum_remaining_stack");
        assert_eq!(instantiated, expected["instantiation"]);
    }
    if id == "overload" {
        let initializer = view.node(nodes["combined"]).unwrap().initializer().unwrap();
        let symbol = op.get_symbol_at_location(initializer).unwrap().unwrap();
        let symbol = op.symbol(symbol).unwrap();
        let name = String::from_utf8(symbol.name_bytes().to_vec()).unwrap();
        let declarations = symbol.declarations().len();
        let ty = op.get_type_at_location(initializer).unwrap();
        let signatures = op.signatures_of_type(ty, false).unwrap().len();
        assert_eq!(
            json!({"name":name,"declarations":declarations,"signatures":signatures}),
            expected["overload_symbol"]
        );
    }
    let queries = request["queries"].as_array().unwrap();
    let types = queries
        .iter()
        .map(|q| {
            op.get_type_at_location(
                view.node(nodes[q.as_str().unwrap()])
                    .unwrap()
                    .name()
                    .unwrap(),
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        json!(types
            .iter()
            .map(|a| types.iter().map(|b| a == b).collect::<Vec<_>>())
            .collect::<Vec<_>>()),
        expected["same"]
    );
    for (index, ty) in types.iter().enumerate() {
        let display = op
            .type_to_string_at(*ty, Some(nodes[queries[index].as_str().unwrap()]), 0)
            .unwrap();
        assert_eq!(
            display.as_bytes(),
            expected["display"][index].as_str().unwrap().as_bytes(),
            "{id} query {index}"
        );
    }
    if request["mode"] == "higher_order" {
        assert_eq!(
            op.c2_higher_order_contract(types[0]).unwrap(),
            expected["higher_order"]
        );
    }
}

fn alias_keys_match_native() {
    const REQUESTS: &str = include_str!("fixtures/c2/alias_instantiation/requests.json");
    const OBSERVED: &str = include_str!("fixtures/c2/alias_instantiation/observations.json");
    let requests: Value = serde_json::from_str(REQUESTS).unwrap();
    let observed: Value = serde_json::from_str(OBSERVED).unwrap();
    let provenance: Value = serde_json::from_str(include_str!(
        "fixtures/c2/alias_instantiation/provenance.json"
    ))
    .unwrap();
    let hash = |b: &[u8]| format!("{:x}", Sha256::digest(b));
    let pin: Value = serde_json::from_str(include_str!("../../../data/upstream.json")).unwrap();
    assert_eq!(provenance["pin"], pin["pin"]);
    assert_eq!(provenance["request_sha256"], hash(REQUESTS.as_bytes()));
    assert_eq!(observed["request_sha256"], provenance["request_sha256"]);
    assert_eq!(provenance["output_sha256"], hash(OBSERVED.as_bytes()));
    for request in requests.as_array().unwrap() {
        let expected = observed["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == request["id"])
            .unwrap();
        let program = support::program(&[("/main.ts", request["source"].as_str().unwrap())]);
        let (_counters, _generation, owner) = support::checker(&program);
        let mut op = owner.operation().unwrap();
        let nodes = declarations(&program);
        let file = program.file(b"/main.ts").unwrap();
        let view = file.bound().view().ast();
        let alias_node = view
            .node(nodes[request["alias"].as_str().unwrap()])
            .unwrap()
            .name()
            .unwrap();
        let alias = op.get_symbol_at_location(alias_node).unwrap().unwrap();
        op.get_declared_type_of_symbol(alias).unwrap();
        let snapshot = |op: &tsr_checker::Operation<'_>| json!({"entries":op.alias_instantiation_cache_entries(alias).unwrap(),"instantiations":op.relation_state()["instantiations"]});
        assert_eq!(snapshot(&op), expected["baseline"]);
        let mut types = Vec::new();
        for (index, name) in request["order"].as_array().unwrap().iter().enumerate() {
            assert_eq!(snapshot(&op), expected["queries"][index]["before"]);
            types.push(
                op.get_type_at_location(
                    view.node(nodes[name.as_str().unwrap()])
                        .unwrap()
                        .name()
                        .unwrap(),
                )
                .unwrap(),
            );
            assert_eq!(snapshot(&op), expected["queries"][index]["after"]);
        }
        assert_eq!(
            json!(types
                .iter()
                .map(|a| types.iter().map(|b| a == b).collect::<Vec<_>>())
                .collect::<Vec<_>>()),
            expected["same"]
        );
    }
}

/// 1: Go newInferenceContext, cloneInferenceContext, inferTypes and inference mappers.
#[test]
fn inference_fixing_clone_and_nonfixing_match_native() {
    observe("inference", None);
}
/// 2: Go getObjectTypeInstantiation/getTypeAliasInstantiation/getConditionalTypeInstantiation/getIndexedAccessType.
#[test]
fn object_alias_conditional_and_indexed_keys_preserve_identity() {
    observe("identity", None);
    alias_keys_match_native();
}
/// 4: Go resolveCall/getCandidateForOverloadFailure over bundled Array.from overloads.
#[test]
fn library_overload_failure_preserves_chain_related_info_and_combined_symbol() {
    observe("overload", None);
}
/// 5: Go pushContextualType/popContextualType and repeated inference passes.
#[test]
fn contextual_errors_restore_argument_context_and_inference_passes() {
    observe("context", None);
    observe("context_cached", None);
}
/// 6: Go instantiateTypeWithSingleGenericCallSignature/getPermissiveInstantiation/getInferredType.
#[test]
fn higher_order_hoisting_and_permissive_wildcard_have_native_defaults() {
    observe("higher_order", None);
}
/// 7: Go inferTypeForHomomorphicMappedType/createReverseMappedType/inferReverseMappedType.
#[test]
fn reverse_mapped_object_and_array_elements_match_native() {
    observe("reverse_mapped", None);
}
/// 9: Go instantiateTypeWithAlias executes the real depth100 branch; recovery is a later source query.
#[test]
fn actual_instantiation_depth_grows_small_stack_and_preserves_reuse() {
    const STACK: usize = 256 * 1024;
    std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(|| observe("instantiation_depth", Some(STACK)))
        .unwrap()
        .join()
        .unwrap();
}

#[cfg(feature = "creation-trace")]
#[path = "support/c2_order_contract.rs"]
mod order_contract;

#[path = "support/c2_variance_limits.rs"]
mod variance_limits;

#[path = "support/c2_cross_product_limits.rs"]
mod cross_product_limits;
