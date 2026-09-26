//! B18: awaited operand compatibility and the exact diagnostic/hint spans.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tsr_arena::{CheckerIdentity, Counters, Generation, NodeId};
use tsr_checker::CheckerOwner;
use tsr_compiler::{FileCache, Program, ProgramCheckerHost, ProgramOptions};
use tsr_core::{CompilerOptions, ScriptTarget, Tristate};
use tsr_jsstring::JsString;

const SOURCE: &str = include_str!("fixtures/c2/awaited_operators.ts");
const REQUEST: &str = include_str!("fixtures/c2/awaited_operators.requests.json");
const NATIVE: &str = include_str!("fixtures/c2/awaited_operators.native.json");
const PROVENANCE: &str = include_str!("fixtures/c2/awaited_operators.provenance.json");

fn diagnostic_record(diagnostic: &tsr_ast::Diagnostic, source: NodeId) -> Value {
    assert_eq!(diagnostic.file, Some(source));
    let message =
        String::from_utf8(tsr_compiler::diagnostic_writer::flattened(diagnostic, b"\n").unwrap())
            .unwrap();
    json!({
        "file": "/awaited_operators.ts", "pos": diagnostic.loc.pos(),
        "end": diagnostic.loc.end(), "code": diagnostic.code,
        "category": diagnostic.category, "message": message,
        "chain": diagnostic.message_chain.iter().map(|d| diagnostic_record(d, source)).collect::<Vec<_>>(),
        "related": diagnostic.related_information.iter().map(|d| diagnostic_record(d, source)).collect::<Vec<_>>()
    })
}

#[test]
fn awaited_operator_errors_match_native_compatibility_and_related_spans() {
    let request: Value = serde_json::from_str(REQUEST).unwrap();
    let native: Value = serde_json::from_str(NATIVE).unwrap();
    let provenance: Value = serde_json::from_str(PROVENANCE).unwrap();
    let pin: Value = serde_json::from_str(include_str!("../../../data/upstream.json")).unwrap();
    assert_eq!(request["source"], SOURCE);
    assert_eq!(provenance["pin"], pin["pin"]);
    let request_hash = format!("{:x}", Sha256::digest(REQUEST.as_bytes()));
    assert_eq!(provenance["request_sha256"], request_hash);
    assert_eq!(native["request_sha256"], request_hash);
    assert_eq!(
        provenance["output_sha256"],
        format!("{:x}", Sha256::digest(NATIVE.as_bytes()))
    );
    assert_eq!(
        provenance["source_sha256"],
        format!(
            "{:x}",
            Sha256::digest(include_bytes!(
                "fixtures/c2/awaited_operators_oracle_test.go"
            ))
        )
    );
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    fs.insert_loaded(b"/awaited_operators.ts", SOURCE.as_bytes());
    let counters = Counters::new();
    let program = Arc::new(
        Program::load(
            ProgramOptions {
                config: tsr_tsoptions::ParsedCommandLine::new(
                    CompilerOptions {
                        target: ScriptTarget::ESNEXT,
                        strict: Tristate::TRUE,
                        ..Default::default()
                    },
                    vec![JsString::from_bytes(b"/awaited_operators.ts".as_slice())],
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
    let source = program.file(b"/awaited_operators.ts").unwrap().source();
    let diagnostics = op.semantic_diagnostics(source).unwrap();
    let observed = diagnostics
        .iter()
        .map(|d| diagnostic_record(d, source))
        .collect::<Vec<_>>();
    // Includes expressions awaiting either/both sides and negative controls:
    // incompatible awaited operands, invalid thenables, and unchanged types.
    assert_eq!(json!(observed), native["diagnostics"]);
}
