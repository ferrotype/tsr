//! Native display length accounting across recursive conditional types.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tsr_arena::{CheckerIdentity, Counters, Generation};
use tsr_checker::CheckerOwner;
use tsr_compiler::{FileCache, Program, ProgramCheckerHost, ProgramOptions};
use tsr_core::{CompilerOptions, ScriptTarget, Tristate};
use tsr_jsstring::JsString;

const SOURCE: &str = include_str!("fixtures/c2/elision/recursive_conditional.ts");
const REQUEST: &str = include_str!("fixtures/c2/elision/requests.json");
const NATIVE: &str = include_str!("fixtures/c2/elision/observations.json");
const PROVENANCE: &str = include_str!("fixtures/c2/elision/provenance.json");

#[test]
fn recursive_conditional_display_completes_and_recovers() {
    let request: Value = serde_json::from_str(REQUEST).unwrap();
    let native: Value = serde_json::from_str(NATIVE).unwrap();
    let provenance: Value = serde_json::from_str(PROVENANCE).unwrap();
    let pin: Value = serde_json::from_str(include_str!("../../../data/upstream.json")).unwrap();
    assert_eq!(request["source"], SOURCE);
    assert_eq!(provenance["pin"], pin["pin"]);
    // The checker-unit branch matrix uses these same independently captured
    // observations; bind its request, observer and output here as well.
    let branch_provenance: Value =
        serde_json::from_str(include_str!("fixtures/c2/elision/branches.provenance.json")).unwrap();
    let branch_output = include_bytes!("fixtures/c2/elision/branches.observations.json");
    let branch_observation: Value = serde_json::from_slice(branch_output).unwrap();
    assert_eq!(branch_provenance["pin"], pin["pin"]);
    for (field, bytes) in [
        (
            "request_sha256",
            include_bytes!("fixtures/c2/elision/branches.requests.json").as_slice(),
        ),
        (
            "source_sha256",
            include_bytes!("fixtures/c2/elision/branches_oracle_test.go").as_slice(),
        ),
        ("output_sha256", branch_output.as_slice()),
    ] {
        assert_eq!(
            branch_provenance[field],
            format!("{:x}", Sha256::digest(bytes))
        );
    }
    assert_eq!(
        branch_observation["request_sha256"],
        branch_provenance["request_sha256"]
    );

    let hash = format!("{:x}", Sha256::digest(REQUEST.as_bytes()));
    assert_eq!(provenance["request_sha256"], hash);
    assert_eq!(native["request_sha256"], hash);
    assert_eq!(
        provenance["output_sha256"],
        format!("{:x}", Sha256::digest(NATIVE.as_bytes()))
    );
    assert_eq!(
        provenance["source_sha256"],
        format!(
            "{:x}",
            Sha256::digest(include_bytes!("fixtures/c2/elision/oracle_test.go"))
        )
    );
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    fs.insert_loaded(b"/recursive_conditional.ts", SOURCE.as_bytes());
    let counters = Counters::new();
    let program = Arc::new(
        Program::load(
            ProgramOptions {
                config: tsr_tsoptions::ParsedCommandLine::new(
                    CompilerOptions {
                        target: ScriptTarget::ES2015,
                        no_error_truncation: Tristate::TRUE,
                        skip_default_lib_check: Tristate::TRUE,
                        ..Default::default()
                    },
                    vec![JsString::from_bytes(
                        b"/recursive_conditional.ts".as_slice(),
                    )],
                ),
                host: Arc::new(tsr_bundled::BundledFs::new(Arc::new(fs.finish()))),
                current_directory: JsString::from_bytes(b"/".as_slice()),
                default_library_path: JsString::from_bytes(tsr_bundled::LIB_PATH),
                skip_module_resolution: false,
            },
            &mut FileCache::new(),
            &counters,
        )
        .unwrap(),
    );
    let owner = Arc::new(
        CheckerOwner::for_program(
            CheckerIdentity::new(Generation::new(&counters), &counters),
            &counters,
            Arc::new(ProgramCheckerHost::new(program.clone())),
        )
        .unwrap(),
    );
    let mut op = owner.operation().unwrap();
    let file = program.file(b"/recursive_conditional.ts").unwrap();
    let source = file.source();
    op.semantic_diagnostics(source).unwrap();
    let view = file.bound().view().ast();
    let statements = view.node(source).unwrap().statements(view).unwrap();
    let mut observed = Vec::new();
    for query in request["queries"].as_array().unwrap() {
        let name = query.as_str().unwrap();
        let declaration = view
            .node_slice(statements)
            .unwrap()
            .iter()
            .flatten()
            .find_map(|id| {
                let node = view.node(id).unwrap();
                let n = node.name()?;
                (view.node_text(n).unwrap().as_bytes() == name.as_bytes()).then_some(n)
            })
            .unwrap();
        let ty = op.get_type_at_location(declaration).unwrap();
        let text = op
            .type_to_string(
                ty,
                tsr_checker::type_format_flags::ALLOW_UNIQUE_ES_SYMBOL_TYPE
                    | tsr_checker::type_format_flags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE,
            )
            .unwrap();
        observed.push(json!({"declaration": name, "bytes": text.as_bytes().len(),
            "sha256": format!("{:x}", Sha256::digest(text.as_bytes()))}));
    }
    // The recursive query must reach the same truncation boundary, including
    // every byte before it; the following query also verifies context recovery.
    assert_eq!(json!(observed), native["queries"]);
}
