//! Native public-checker contracts for JSDoc signatures and CommonJS aliases.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{ops::ControlFlow, sync::Arc};
use tsr_arena::{CheckerIdentity, Counters, Generation, NodeId};
use tsr_ast::{AstView, ChildVisitor, SyntaxKind as K};
use tsr_checker::CheckerOwner;
use tsr_compiler::{FileCache, Program, ProgramCheckerHost, ProgramOptions};
use tsr_core::{CompilerOptions, ModuleKind, ScriptTarget, Tristate};
use tsr_jsstring::JsString;

const REQUESTS: &str = include_str!("fixtures/c2/js_full_signatures/requests.json");
const NATIVE: &str = include_str!("fixtures/c2/js_full_signatures/native.json");
const PROVENANCE: &str = include_str!("fixtures/c2/js_full_signatures/provenance.json");

fn diagnostic(program: &Program, d: &tsr_ast::Diagnostic) -> Value {
    let file = d.file.map(|source| {
        let file = program
            .files()
            .iter()
            .find(|f| f.source() == source)
            .unwrap();
        String::from_utf8(
            file.bound()
                .view()
                .source_file()
                .unwrap()
                .file_name()
                .to_vec(),
        )
        .unwrap()
    });
    json!({"file":file,"pos":d.loc.pos(),"end":d.loc.end(),"code":d.code,"category":d.category,
            "message":String::from_utf8(tsr_compiler::diagnostic_writer::localized(d).unwrap()).unwrap(),
            "chain":d.message_chain.iter().map(|d| diagnostic(program, d)).collect::<Vec<_>>(),
            "related":d.related_information.iter().map(|d| diagnostic(program, d)).collect::<Vec<_>>()})
}

fn names(view: AstView<'_>, source: NodeId) -> Vec<NodeId> {
    struct Walk<'a> {
        view: AstView<'a>,
        names: Vec<NodeId>,
    }
    impl ChildVisitor for Walk<'_> {
        fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
            let read = self.view.node(node).unwrap();
            if matches!(
                read.kind().known(),
                Some(K::VariableDeclaration | K::FunctionDeclaration)
            ) {
                if let Some(name) = read.name() {
                    self.names.push(name);
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
    let mut walk = Walk {
        view,
        names: Vec::new(),
    };
    let _ = walk.visit_node(source);
    walk.names
}

fn observe(id: &str) {
    let requests: Value = serde_json::from_str(REQUESTS).unwrap();
    let native: Value = serde_json::from_str(NATIVE).unwrap();
    let provenance: Value = serde_json::from_str(PROVENANCE).unwrap();
    let pin: Value = serde_json::from_str(include_str!("../../../data/upstream.json")).unwrap();
    let sha = |bytes: &[u8]| format!("{:x}", Sha256::digest(bytes));
    assert_eq!(provenance["pin"], pin["pin"]);
    assert_eq!(provenance["request_sha256"], sha(REQUESTS.as_bytes()));
    assert_eq!(native["request_sha256"], provenance["request_sha256"]);
    assert_eq!(provenance["output_sha256"], sha(NATIVE.as_bytes()));
    for (name, bytes) in [
        (
            "oracle_test.go",
            include_bytes!("fixtures/c2/js_full_signatures/oracle_test.go").as_slice(),
        ),
        (
            "regenerate.py",
            include_bytes!("fixtures/c2/js_full_signatures/regenerate.py").as_slice(),
        ),
    ] {
        assert_eq!(provenance["observer_sources"][name], sha(bytes));
    }
    let request = requests
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == id)
        .unwrap();
    let expected = native["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == id)
        .unwrap();
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    for (name, source) in request["files"].as_object().unwrap() {
        fs.insert_loaded(name.as_bytes(), source.as_str().unwrap().as_bytes());
    }
    let roots = request["roots"].as_array().unwrap();
    let counters = Counters::new();
    let program = Arc::new(
        Program::load(
            ProgramOptions {
                config: tsr_tsoptions::ParsedCommandLine::new(
                    CompilerOptions {
                        target: ScriptTarget::ES2015,
                        module: ModuleKind::COMMON_JS,
                        allow_js: Tristate::TRUE,
                        check_js: Tristate::TRUE,
                        no_emit: Tristate::TRUE,
                        skip_default_lib_check: Tristate::TRUE,
                        ..Default::default()
                    },
                    roots
                        .iter()
                        .map(|n| JsString::from_bytes(n.as_str().unwrap().as_bytes()))
                        .collect(),
                ),
                host: Arc::new(tsr_bundled::BundledFs::new(Arc::new(fs.finish()))),
                current_directory: JsString::from_bytes(
                    request["cwd"].as_str().unwrap().as_bytes(),
                ),
                default_library_path: JsString::from_bytes(tsr_bundled::LIB_PATH),
                skip_module_resolution: false,
            },
            &mut FileCache::new(),
            &counters,
        )
        .unwrap(),
    );
    let owner = CheckerOwner::for_program(
        CheckerIdentity::new(Generation::new(&counters), &counters),
        &counters,
        Arc::new(ProgramCheckerHost::new(program.clone())),
    )
    .unwrap();
    let owner = Arc::new(owner);
    let mut op = owner.operation().unwrap();
    // Repeat the same schedule to exercise cached aliases and completed members.
    for _ in 0..2 {
        let mut ds = Vec::new();
        let mut queries = Vec::new();
        for root in roots {
            let name = root.as_str().unwrap();
            let file = program.file(name.as_bytes()).unwrap();
            ds.extend(op.semantic_diagnostics(file.source()).unwrap());
            let view = file.bound().view().ast();
            for node in names(view, file.source()) {
                let read = view.node(node).unwrap();
                let ty = op.get_type_at_location(node).unwrap();
                queries.push(json!({"file":name,"pos":read.pos(),"end":read.end(),
                    "text":String::from_utf8(op.type_to_string(ty,0).unwrap().as_bytes().to_vec()).unwrap()}));
            }
        }
        assert_eq!(
            json!(ds
                .iter()
                .map(|d| diagnostic(&program, d))
                .collect::<Vec<_>>()),
            expected["diagnostics"],
            "{id} diagnostics"
        );
        assert_eq!(json!(queries), expected["queries"], "{id} public types");
    }
}

#[test]
fn require_alias_reaches_generic_class() {
    observe("generic_export");
}
#[test]
fn alias_chain_preserves_type_and_namespace_meanings() {
    observe("alias_meanings");
}
#[test]
fn full_signature_supplies_this_parameter() {
    observe("this_annotations");
}
#[test]
fn full_signature_supplies_return_checks_and_error_span() {
    observe("return_annotations");
}
#[test]
fn commonjs_function_aliases_match_native() {
    observe("cjs_function_aliases");
}
#[test]
fn commonjs_members_publish_before_alias_queries() {
    observe("member_publication");
}
