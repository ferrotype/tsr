//! Three source-audit regressions, checked against a pinned native overlay.
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

const REQUESTS: &str = include_str!("fixtures/c2/audit_gaps/requests.json");
const NATIVE: &str = include_str!("fixtures/c2/audit_gaps/native.json");
const PROVENANCE: &str = include_str!("fixtures/c2/audit_gaps/provenance.json");

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
            include_bytes!("fixtures/c2/audit_gaps/oracle_bridge.go").as_slice(),
        ),
        (
            "oracle_test.go",
            include_bytes!("fixtures/c2/audit_gaps/oracle_test.go").as_slice(),
        ),
        (
            "regenerate.py",
            include_bytes!("fixtures/c2/audit_gaps/regenerate.py").as_slice(),
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
                        | K::TypeParameter
                        | K::ClassDeclaration
                        | K::InterfaceDeclaration
                        | K::ImportEqualsDeclaration
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
        "reports_deprecated":d.reports_deprecated,"chain":d.message_chain.iter().map(|d|diagnostic(program,d)).collect::<Vec<_>>(),
        "related":d.related_information.iter().map(|d|diagnostic(program,d)).collect::<Vec<_>>()})
}

fn observe(id: &str) {
    let (request, expected) = fixtures(id);
    let files = request["files"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, text)| (name.as_str(), text.as_str().unwrap()))
        .collect::<Vec<_>>();
    let program = support::program(&files);
    let nodes = declarations(&program);
    let file = program.file(b"/main.ts").unwrap();
    let view = file.bound().view().ast();
    let (_counters, _generation, owner) = support::checker(&program);
    let mut op = owner.operation().unwrap();
    if id == "mapped_property_cycle" {
        assert_eq!(
            op.c2_mapped_property_cycle_contract(
                view.node(nodes["mapped"]).unwrap().name().unwrap()
            )
            .unwrap(),
            expected["cycle"]
        );
    }
    if id == "type_parameter_helpers" {
        let mut aliases = Vec::new();
        for name in ["Plain", "Erased", "Defaults", "Bound"] {
            let symbol = op
                .get_symbol_at_location(view.node(nodes[name]).unwrap().name().unwrap())
                .unwrap()
                .unwrap();
            let parameters = op.get_type_alias_type_parameters(symbol).unwrap();
            let again = op.get_type_alias_type_parameters(symbol).unwrap();
            let names = parameters
                .iter()
                .map(|&p| {
                    String::from_utf8(op.type_to_string(p, 0).unwrap().as_bytes().to_vec()).unwrap()
                })
                .collect::<Vec<_>>();
            aliases.push(json!({"name":name,"parameters":names,"same":parameters==again}));
        }
        let capabilities = [
            "Outer",
            "Face",
            "fun",
            "arrow",
            "functionExpr",
            "plain",
            "Plain",
            "Alias",
        ]
        .map(|name| nodes[name]);
        let parameters = ["P", "D", "B", "K", "R"].map(|name| nodes[name]);
        let mut observed = op
            .c2_type_parameter_helper_contract(
                &capabilities,
                &parameters,
                nodes["Outer"],
                nodes["Alias"],
                view.node(nodes["instance"]).unwrap().name().unwrap(),
            )
            .unwrap();
        observed["aliases"] = json!(aliases);
        assert_eq!(observed, expected["parameters"]);
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
    let suggestions = op.recorded_suggestions(file.source()).unwrap();
    assert_eq!(
        json!(suggestions
            .iter()
            .map(|d| diagnostic(&program, d))
            .collect::<Vec<_>>()),
        expected["suggestions"],
        "{id} suggestions"
    );
    let types = request["queries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| {
            op.get_type_at_location(
                view.node(nodes[name.as_str().unwrap()])
                    .unwrap()
                    .name()
                    .unwrap(),
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    let identity = types
        .iter()
        .map(|&a| {
            types
                .iter()
                .map(|&b| {
                    op.is_type_related_to(a, b, tsr_checker::RelationKind::Identity)
                        .unwrap()
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    assert_eq!(json!(identity), expected["identity"], "{id} identity");
    let later = op
        .get_type_at_location(view.node(nodes["later"]).unwrap().name().unwrap())
        .unwrap();
    assert_eq!(
        String::from_utf8(op.type_to_string(later, 0).unwrap().as_bytes().to_vec()).unwrap(),
        expected["later"]
    );
}
#[test]
fn signature_identity_reads_direct_constraints() {
    observe("signature_identity");
}
#[test]
fn deprecated_type_references_and_imports_keep_suggestions() {
    observe("deprecated_types");
}
#[test]
fn completed_intermediate_property_stops_mapped_cycle_search() {
    observe("mapped_property_cycle");
}

#[test]
fn infer_keeps_constraints_of_any_valued_alias() {
    observe("infer_any_alias_constraint");
}

#[test]
fn type_parameter_queries_and_declaration_predicates_match_native() {
    observe("type_parameter_helpers");
}
