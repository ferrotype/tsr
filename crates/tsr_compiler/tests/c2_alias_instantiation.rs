//! Pinned getTypeAliasInstantiation observations: cache work is independent of
//! returned identity. The fixture observer reads links and pointer equality only.
#![cfg(feature = "relation-probe")]

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tsr_arena::{CheckerIdentity, Counters, Generation, NodeId};
use tsr_ast::SyntaxKind as K;
use tsr_checker::{CheckerOwner, Error};
use tsr_compiler::{FileCache, Program, ProgramCheckerHost, ProgramOptions};
use tsr_core::{CompilerOptions, ModuleKind, ScriptTarget, Tristate};
use tsr_jsstring::JsString;

const REQUESTS: &str = include_str!("fixtures/c2/alias_instantiation/requests.json");
const OBSERVATIONS: &str = include_str!("fixtures/c2/alias_instantiation/observations.json");
const PROVENANCE: &str = include_str!("fixtures/c2/alias_instantiation/provenance.json");

fn fixtures() -> (Value, Value) {
    let requests: Value = serde_json::from_str(REQUESTS).unwrap();
    let observed: Value = serde_json::from_str(OBSERVATIONS).unwrap();
    let provenance: Value = serde_json::from_str(PROVENANCE).unwrap();
    let pin: Value = serde_json::from_str(include_str!("../../../data/upstream.json")).unwrap();
    let hash = |bytes: &[u8]| format!("{:x}", Sha256::digest(bytes));
    assert_eq!(provenance["pin"], pin["pin"]);
    assert_eq!(provenance["request_sha256"], hash(REQUESTS.as_bytes()));
    assert_eq!(observed["request_sha256"], provenance["request_sha256"]);
    assert_eq!(provenance["output_sha256"], hash(OBSERVATIONS.as_bytes()));
    for (name, bytes) in [
        (
            "oracle_bridge.go",
            include_bytes!("fixtures/c2/alias_instantiation/oracle_bridge.go").as_slice(),
        ),
        (
            "oracle_test.go",
            include_bytes!("fixtures/c2/alias_instantiation/oracle_test.go").as_slice(),
        ),
    ] {
        assert_eq!(provenance["observer_sources"][name], hash(bytes));
    }
    let ids = json!([
        "object-a-first",
        "object-b-first",
        "intrinsic-a-first",
        "intrinsic-b-first",
        "dependent-a-first",
        "dependent-b-first"
    ]);
    assert_eq!(
        json!(
            requests
                .as_array()
                .unwrap()
                .iter()
                .map(|r| &r["id"])
                .collect::<Vec<_>>()
        ),
        ids
    );
    assert_eq!(
        json!(
            observed["rows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| &r["id"])
                .collect::<Vec<_>>()
        ),
        ids
    );
    (requests, observed)
}

fn program(source: &str) -> Arc<Program> {
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    fs.insert_loaded(b"/main.ts", source.as_bytes());
    Arc::new(
        Program::load(
            ProgramOptions {
                config: tsr_tsoptions::ParsedCommandLine::new(
                    CompilerOptions {
                        target: ScriptTarget::ESNEXT,
                        module: ModuleKind::ESNEXT,
                        strict: Tristate::TRUE,
                        skip_lib_check: Tristate::TRUE,
                        ..Default::default()
                    },
                    vec![JsString::from_bytes(b"/main.ts".as_slice())],
                ),
                host: Arc::new(tsr_bundled::BundledFs::new(Arc::new(fs.finish()))),
                current_directory: JsString::from_bytes(b"/".as_slice()),
                default_library_path: JsString::from_bytes(tsr_bundled::LIB_PATH),
                skip_module_resolution: false,
            },
            &mut FileCache::new(),
            &Counters::new(),
        )
        .unwrap(),
    )
}

fn checker(program: &Arc<Program>) -> Arc<CheckerOwner> {
    let counters = Counters::new();
    let generation = Generation::new(&counters);
    Arc::new(
        CheckerOwner::for_program(
            CheckerIdentity::new(generation, &counters),
            &counters,
            Arc::new(ProgramCheckerHost::new(program.clone())),
        )
        .unwrap(),
    )
}

fn declaration(program: &Program, name: &str) -> (NodeId, NodeId) {
    let file = program.file(b"/main.ts").unwrap();
    let view = file.bound().view().ast();
    let statements = view.node(file.source()).unwrap().statements(view).unwrap();
    for statement in view.node_slice(statements).unwrap().iter().flatten() {
        let read = view.node(statement).unwrap();
        let nodes = if read.kind() == K::VariableStatement {
            let list = read
                .as_variable_statement()
                .unwrap()
                .declaration_list()
                .unwrap();
            let list = view
                .node(list)
                .unwrap()
                .as_variable_declaration_list()
                .unwrap()
                .declarations()
                .unwrap();
            view.node_slice(view.list(list).unwrap().nodes())
                .unwrap()
                .iter()
                .flatten()
                .collect::<Vec<_>>()
        } else {
            vec![statement]
        };
        for node in nodes {
            if let Some(id) = view.node(node).unwrap().name() {
                if view.node_text(id).unwrap().as_bytes() == name.as_bytes() {
                    return (node, id);
                }
            }
        }
    }
    panic!("missing declaration {name}")
}

fn check_native(prefix: &str) {
    let (requests, observed) = fixtures();
    for request in requests
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["id"].as_str().unwrap().starts_with(prefix))
    {
        let expected = observed["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == request["id"])
            .unwrap();
        let program = program(request["source"].as_str().unwrap());
        let owner = checker(&program);
        let mut op = owner.operation().unwrap();
        let (_, alias_name) = declaration(&program, request["alias"].as_str().unwrap());
        let alias = op.get_symbol_at_location(alias_name).unwrap().unwrap();
        op.get_declared_type_of_symbol(alias).unwrap();
        let snapshot = |op: &tsr_checker::Operation<'_>| {
            json!({
                "entries": op.alias_instantiation_cache_entries(alias).unwrap(),
                "instantiations": op.relation_state()["instantiations"],
            })
        };
        assert_eq!(
            snapshot(&op),
            expected["baseline"],
            "{} baseline",
            request["id"]
        );
        let mut types = Vec::new();
        let mut nodes = Vec::new();
        for (index, name) in request["order"].as_array().unwrap().iter().enumerate() {
            let (node, name_node) = declaration(&program, name.as_str().unwrap());
            assert_eq!(snapshot(&op), expected["queries"][index]["before"]);
            types.push(op.get_type_at_location(name_node).unwrap());
            nodes.push(node);
            assert_eq!(
                snapshot(&op),
                expected["queries"][index]["after"],
                "{} query {name}",
                request["id"]
            );
        }
        let same: Vec<Vec<bool>> = types
            .iter()
            .map(|a| types.iter().map(|b| a == b).collect())
            .collect();
        assert_eq!(json!(same), expected["same"]);
        // Public display follows every measured query, matching the native driver.
        for (index, (&ty, &node)) in types.iter().zip(&nodes).enumerate() {
            let display = op.type_to_string_at(ty, Some(node), 0).unwrap();
            assert_eq!(
                display.as_bytes(),
                expected["display"][index].as_str().unwrap().as_bytes()
            );
        }
        drop(op);
        let diagnostics_owner = checker(&program);
        let mut diagnostics_op = diagnostics_owner.operation().unwrap();
        let diagnostics = diagnostics_op
            .semantic_diagnostics(program.file(b"/main.ts").unwrap().source())
            .unwrap();
        let actual: Vec<_> = diagnostics
            .iter()
            .map(|d| json!({"code":d.code,"pos":d.loc.pos(),"end":d.loc.end()}))
            .collect();
        let expected: Vec<_> = expected["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| json!({"code":d["code"],"pos":d["pos"],"end":d["end"]}))
            .collect();
        assert_eq!(actual, expected);
    }
}

#[test]
fn explicit_and_defaulted_arguments_have_distinct_cache_entries_but_share_type() {
    check_native("object-");
}

#[test]
fn intrinsic_dispatch_uses_the_unfilled_argument_count() {
    check_native("intrinsic-");
}

#[test]
fn a_cache_hit_does_not_instantiate_dependent_defaults_again() {
    check_native("dependent-");
}

#[test]
fn alias_cache_observer_is_read_only_and_rejects_another_checker() {
    let program = program("type Box<T> = { value: T };\n");
    let owner = checker(&program);
    let foreign_owner = checker(&program);
    let mut op = owner.operation().unwrap();
    let (_, name) = declaration(&program, "Box");
    let symbol = op.get_symbol_at_location(name).unwrap().unwrap();
    let before = op.relation_state();
    for _ in 0..2 {
        assert_eq!(op.alias_instantiation_cache_entries(symbol).unwrap(), 0);
        assert_eq!(op.relation_state(), before);
    }
    let foreign = foreign_owner.operation().unwrap();
    assert_eq!(
        foreign.alias_instantiation_cache_entries(symbol),
        Err(Error::Arena(tsr_arena::Error::WrongOwner))
    );
    op.get_declared_type_of_symbol(symbol).unwrap();
    assert_eq!(op.alias_instantiation_cache_entries(symbol).unwrap(), 1);
}
