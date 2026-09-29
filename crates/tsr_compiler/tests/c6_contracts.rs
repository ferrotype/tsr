//! Phase 2 C6 direct contracts (docs/PHASE2-C6-plan.md, C6.9), each over
//! production entry points with its pinned Go counterpart named.
#[allow(dead_code)]
#[path = "../../../tools/s08/p5/baseline/mod.rs"]
mod baseline;
#[allow(dead_code)]
#[path = "../../../tools/s08/p5/corpus.rs"]
mod corpus;
#[allow(dead_code)]
#[path = "../../../tools/s08/p5/errors.rs"]
mod errors;
#[allow(dead_code)]
#[path = "../../../tools/s08/p4/executor.rs"]
mod executor;
#[allow(dead_code)]
#[path = "../../../tools/s08/p5/paths.rs"]
mod paths;
#[allow(dead_code)]
#[path = "../../../tools/phase2/subtests.rs"]
mod subtests;

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use tsr_arena::{CheckerIdentity, Counters, Generation, NodeId};
use tsr_checker::{
    CheckerOwner, CheckerRequest, MemoryTraceSink, TraceLocation, TraceSink, TraceTypeRecord,
    TraceValue, Tracer,
};
use tsr_compiler::{
    CheckerAssociationPlan, CompilerCheckerPool, FileCache, Program, ProgramCheckerHost,
    ProgramOptions,
};
use tsr_core::{CompilerOptions, Tristate};
use tsr_jsstring::JsString;

const TRACE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/c6/trace");

fn program(files: &[(&[u8], &[u8])], options: CompilerOptions) -> (Arc<Program>, Counters) {
    program_in(files, options, Tristate::UNKNOWN)
}

/// A program with its own single-threaded setting (the pin's
/// `ProgramOptions.SingleThreaded`).
fn program_in(
    files: &[(&[u8], &[u8])],
    options: CompilerOptions,
    single_threaded: Tristate,
) -> (Arc<Program>, Counters) {
    let counters = Counters::new();
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    for &(path, text) in files {
        fs.insert_loaded(path, text);
    }
    let program = Program::load(
        ProgramOptions {
            config: tsr_tsoptions::ParsedCommandLine::new(
                options,
                files
                    .iter()
                    .map(|&(path, _)| JsString::from_bytes(path))
                    .collect(),
            ),
            host: Arc::new(fs.finish()),
            current_directory: JsString::from_bytes(b"/".as_slice()),
            default_library_path: JsString::from_bytes(b"/no-default-lib".as_slice()),
            skip_module_resolution: false,
            single_threaded,
        },
        &mut FileCache::new(),
        &counters,
    )
    .unwrap();
    (Arc::new(program), counters)
}

fn checker(
    program: &Arc<Program>,
    counters: &Counters,
    tracer: Option<Tracer>,
) -> Arc<CheckerOwner> {
    let generation = Generation::new(counters);
    Arc::new(
        CheckerOwner::for_program_with_tracer(
            CheckerIdentity::new(generation, counters),
            counters,
            Arc::new(ProgramCheckerHost::new(program.clone())),
            tracer,
        )
        .unwrap(),
    )
}

fn args_json(args: &BTreeMap<String, TraceValue>) -> Value {
    Value::Object(
        args.iter()
            .map(|(key, value)| {
                let value = match value {
                    TraceValue::Int(value) => json!(value),
                    TraceValue::Str(value) => json!(value),
                    TraceValue::Strs(values) => json!(values),
                };
                (key.clone(), value)
            })
            .collect(),
    )
}

fn location_json(location: &TraceLocation) -> Value {
    json!({"path": location.path,
           "start": {"line": location.start.0, "character": location.start.1},
           "end": {"line": location.end.0, "character": location.end.1}})
}

/// A type record as the pin's `types.json` writes it (`omitzero` fields left out).
fn record_json(record: &TraceTypeRecord) -> Value {
    let mut value = serde_json::Map::new();
    value.insert("id".into(), json!(record.id));
    value.insert("flags".into(), json!(record.flags));
    let mut put = |key: &str, field: Option<Value>| {
        if let Some(field) = field {
            value.insert(key.into(), field);
        }
    };
    put(
        "intrinsicName",
        record.intrinsic_name.as_ref().map(|v| json!(v)),
    );
    put("symbolName", record.symbol_name.as_ref().map(|v| json!(v)));
    put("recursionId", record.recursion_id.map(|v| json!(v)));
    put("isTuple", record.is_tuple.then(|| json!(true)));
    let list = |values: &[u32]| (!values.is_empty()).then(|| json!(values));
    put("unionTypes", list(&record.union_types));
    put("intersectionTypes", list(&record.intersection_types));
    put("aliasTypeArguments", list(&record.alias_type_arguments));
    put("keyofType", record.keyof_type.map(|v| json!(v)));
    put(
        "indexedAccessObjectType",
        record.indexed_access_object_type.map(|v| json!(v)),
    );
    put(
        "indexedAccessIndexType",
        record.indexed_access_index_type.map(|v| json!(v)),
    );
    put(
        "conditionalCheckType",
        record.conditional_check_type.map(|v| json!(v)),
    );
    put(
        "conditionalExtendsType",
        record.conditional_extends_type.map(|v| json!(v)),
    );
    put(
        "conditionalTrueType",
        record.conditional_true_type.map(|v| json!(v)),
    );
    put(
        "conditionalFalseType",
        record.conditional_false_type.map(|v| json!(v)),
    );
    put(
        "substitutionBaseType",
        record.substitution_base_type.map(|v| json!(v)),
    );
    put("constraintType", record.constraint_type.map(|v| json!(v)));
    put(
        "instantiatedType",
        record.instantiated_type.map(|v| json!(v)),
    );
    put("typeArguments", list(&record.type_arguments));
    put(
        "referenceLocation",
        record.reference_location.as_ref().map(location_json),
    );
    put(
        "reverseMappedSourceType",
        record.reverse_mapped_source_type.map(|v| json!(v)),
    );
    put(
        "reverseMappedMappedType",
        record.reverse_mapped_mapped_type.map(|v| json!(v)),
    );
    put(
        "reverseMappedConstraintType",
        record.reverse_mapped_constraint_type.map(|v| json!(v)),
    );
    put(
        "evolvingArrayElementType",
        record.evolving_array_element_type.map(|v| json!(v)),
    );
    put(
        "evolvingArrayFinalType",
        record.evolving_array_final_type.map(|v| json!(v)),
    );
    put(
        "destructuringPattern",
        record.destructuring_pattern.as_ref().map(location_json),
    );
    put(
        "firstDeclaration",
        record.first_declaration.as_ref().map(location_json),
    );
    put("display", record.display.as_ref().map(|v| json!(v)));
    Value::Object(value)
}

fn diagnostics(owner: &Arc<CheckerOwner>, sources: &[NodeId]) -> Vec<(i32, i64, i64)> {
    let mut op = owner.operation().unwrap();
    let mut result = Vec::new();
    for &source in sources {
        for diagnostic in op.semantic_diagnostics(source).unwrap() {
            result.push((diagnostic.code, diagnostic.loc.pos(), diagnostic.loc.end()));
        }
    }
    result
}

/// Contract 7, tracing (`checker/tracer.go` over `tracing.Tracing`): the pinned
/// witness checked single-threaded with a deterministic in-memory session
/// yields the pin's checker events and type records (the normalized native
/// `trace.json` and `types_0.json`), and tracing on or off leaves the
/// checker's diagnostics identical.
#[test]
fn tracing_records_the_pins_events_and_types_and_changes_nothing() {
    let native: Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{TRACE}/native.json")).unwrap())
            .unwrap();
    let lib = std::fs::read(format!("{TRACE}/lib.d.ts")).unwrap();
    let source = std::fs::read(format!("{TRACE}/a.ts")).unwrap();
    let options = CompilerOptions {
        no_lib: Tristate::TRUE,
        no_emit: Tristate::TRUE,
        ..Default::default()
    };
    let files: [(&[u8], &[u8]); 2] = [(b"/lib.d.ts", &lib), (b"/a.ts", &source)];

    let (traced_program, counters) = program(&files, options.clone());
    let sink = Arc::new(MemoryTraceSink::new(true));
    let traced = checker(
        &traced_program,
        &counters,
        Some(Tracer::new(sink.clone() as Arc<dyn TraceSink>, 0)),
    );
    let sources: Vec<NodeId> = [b"/lib.d.ts".as_slice(), b"/a.ts"]
        .iter()
        .map(|name| traced_program.file(name).unwrap().source())
        .collect();
    let with_tracing = diagnostics(&traced, &sources);

    let events: Vec<Value> = sink
        .events()
        .iter()
        .map(|event| {
            let mut value =
                json!({"ph": event.ph, "cat": event.phase.as_str(), "name": event.name});
            if !event.args.is_empty() {
                value["args"] = args_json(&event.args);
            }
            value
        })
        .collect();
    assert_eq!(Value::Array(events), native["events"]);

    let recorded = sink.recorded_types();
    assert_eq!(recorded.keys().copied().collect::<Vec<_>>(), vec![0]);
    let records = traced
        .operation()
        .unwrap()
        .trace_type_records(&recorded[&0])
        .unwrap();
    let records: Vec<Value> = records.iter().map(record_json).collect();
    let expected = native["types"].as_array().unwrap();
    if std::env::var_os("C6_TRACE_DUMP").is_some() {
        std::fs::write(
            std::env::var("C6_TRACE_DUMP").unwrap(),
            serde_json::to_string_pretty(&records).unwrap(),
        )
        .unwrap();
    }
    assert_eq!(records.len(), expected.len(), "recorded type count");
    for (actual, expected) in records.iter().zip(expected) {
        assert_eq!(actual, expected);
    }

    let (plain_program, counters) = program(&files, options);
    let plain = checker(&plain_program, &counters, None);
    let sources: Vec<NodeId> = [b"/lib.d.ts".as_slice(), b"/a.ts"]
        .iter()
        .map(|name| plain_program.file(name).unwrap().source())
        .collect();
    assert_eq!(diagnostics(&plain, &sources), with_tracing);
}

/// A session that cancels a token when the checker opens its `at`-th event of
/// one name: a deterministic way to cancel in the middle of a check, through
/// the production trace seam.
struct CancelAt {
    name: &'static str,
    at: usize,
    token: tsr_core::CancellationToken,
    seen: std::sync::atomic::AtomicUsize,
}

impl CancelAt {
    fn new(name: &'static str, at: usize, token: &tsr_core::CancellationToken) -> Arc<Self> {
        Arc::new(Self {
            name,
            at,
            token: token.clone(),
            seen: std::sync::atomic::AtomicUsize::new(0),
        })
    }
    fn seen(&self) -> usize {
        self.seen.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl TraceSink for CancelAt {
    fn push(
        &self,
        _: tsr_checker::TracePhase,
        name: &str,
        _: &tsr_checker::TraceArgs,
        _: bool,
    ) -> u64 {
        if name == self.name
            && self.seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1 == self.at
        {
            self.token.cancel();
        }
        0
    }
    fn pop(&self, _: u64, _: &tsr_checker::TraceArgs) {}
    fn instant(&self, _: tsr_checker::TracePhase, _: &str, _: &tsr_checker::TraceArgs) {}
    fn record_type(&self, _: usize, _: u32) {}
}

fn no_lib() -> CompilerOptions {
    CompilerOptions {
        no_lib: Tristate::TRUE,
        ..Default::default()
    }
}

/// Contract 4, cancellation between statements (checker.go `checkSourceFile`,
/// `checkSourceElements`, `getDiagnostics`, `GetGlobalDiagnostics`,
/// `utilities.go` `isCanceled` and `checkNotCanceled`, nodecopy.go
/// `createRecoveryBoundary`, project/checkerpool.go `createRelease`): a check
/// canceled while its first statement is checked stops at the next statement
/// poll and returns no diagnostics; the checker reports `WasCanceled` and from
/// then on refuses diagnostics, global diagnostics and node building with the
/// pin's message, without retiring its generation; the project pool disposes
/// it at release and serves a fresh checker that checks the file. Through the
/// program API the request's token cancels the check, and its lifetime
/// selects the project pool's diagnostics checker.
#[test]
fn cancellation_between_statements_stops_the_check_and_poisons_the_checker() {
    let text: &[u8] = b"let a: number = \"x\";\nlet b: number = \"y\";\nlet c: number = \"z\";\n";
    let (program, counters) = program(&[(b"/a.ts", text)], no_lib());
    let source = program.file(b"/a.ts").unwrap().source();
    let token = tsr_core::CancellationToken::new();
    let sink = CancelAt::new("checkVariableDeclaration", 1, &token);
    let owner = checker(
        &program,
        &counters,
        Some(Tracer::new(sink.clone() as Arc<dyn TraceSink>, 0)),
    );
    let mut op = owner.operation().unwrap();
    assert!(!op.was_canceled());
    assert_eq!(
        op.semantic_diagnostics_cancellable(source, &token).unwrap(),
        vec![]
    );
    assert_eq!(sink.seen(), 1, "the second statement is never checked");
    assert!(op.was_canceled());
    assert!(owner.was_canceled());
    let refused = op.semantic_diagnostics(source).unwrap_err();
    assert_eq!(refused, tsr_checker::Error::PreviouslyCanceled);
    assert_eq!(refused.to_string(), "Checker was previously cancelled");
    assert_eq!(
        op.global_diagnostics().unwrap_err(),
        tsr_checker::Error::PreviouslyCanceled
    );
    let declaration = {
        let view = program.file(b"/a.ts").unwrap().bound().view().ast();
        let statement = view
            .node_slice(view.node(source).unwrap().statements(view).unwrap())
            .unwrap()
            .get(0)
            .unwrap()
            .unwrap();
        let list = view
            .node(statement)
            .unwrap()
            .data_source()
            .as_variable_statement()
            .unwrap()
            .declaration_list()
            .unwrap();
        let declarations = view
            .node(list)
            .unwrap()
            .data_source()
            .as_variable_declaration_list()
            .unwrap()
            .declarations()
            .unwrap();
        view.node_slice(view.list(declarations).unwrap().nodes())
            .unwrap()
            .get(0)
            .unwrap()
            .unwrap()
    };
    let request = tsr_checker::BuilderRequest {
        enclosing: Some(source),
        flags: 0,
        internal_flags: 0,
    };
    assert_eq!(
        op.node_builder()
            .serialize_type_for_declaration(declaration, None, request)
            .unwrap_err(),
        tsr_checker::Error::PreviouslyCanceled
    );
    drop(op);
    // The generation stays live: a new operation on the owner is granted.
    assert!(owner.operation().is_ok());

    let host: Arc<dyn tsr_checker::CheckerHost> =
        Arc::new(ProgramCheckerHost::new(program.clone()));
    let pool = tsr_project::CheckerPool::for_program(host, &counters, 1);
    let canceled = tsr_core::CancellationToken::new();
    canceled.cancel();
    let checkout = pool.acquire(tsr_project::CheckerSlot::Diagnostics).unwrap();
    let first = checkout.owner().clone();
    assert_eq!(
        checkout
            .operation()
            .unwrap()
            .semantic_diagnostics_cancellable(source, &canceled)
            .unwrap(),
        vec![]
    );
    assert!(first.was_canceled());
    drop(checkout);
    let checkout = pool.acquire(tsr_project::CheckerSlot::Diagnostics).unwrap();
    assert!(
        !Arc::ptr_eq(checkout.owner(), &first),
        "the canceled checker is disposed at release"
    );
    let codes: Vec<i32> = checkout
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap()
        .iter()
        .map(|d| d.code)
        .collect();
    assert_eq!(codes, vec![2322, 2322, 2322]);
    drop(checkout);

    // Through the program API (program.go `GetSemanticDiagnostics(ctx, file)`):
    // the request's token cancels the file's check, which leaves the program
    // only its bind diagnostics (none here) and poisons the file's checker.
    let file = program.file(b"/a.ts").unwrap();
    let request = CheckerRequest {
        cancellation: Some(canceled.clone()),
        ..CheckerRequest::default()
    };
    let checked = tsr_compiler::CheckedProgram::new(program.clone(), &counters, None);
    assert_eq!(
        checked.semantic_diagnostics(&request, Some(file)).unwrap(),
        vec![]
    );
    assert!(matches!(
        checked.semantic_diagnostics(&CheckerRequest::default(), Some(file)),
        Err(tsr_compiler::Error::Checker(
            tsr_checker::Error::PreviouslyCanceled
        ))
    ));
    // The request's lifetime selects a supplied pool's checker: a diagnostics
    // request is served by the project pool's diagnostics checker, which is
    // canceled and then disposed, while its query checker is untouched.
    let host: Arc<dyn tsr_checker::CheckerHost> =
        Arc::new(ProgramCheckerHost::new(program.clone()));
    let project =
        tsr_project::Project::new(tsr_project::CheckerPool::for_program(host, &counters, 1));
    let diagnostics_owner = project
        .pool()
        .acquire(tsr_project::CheckerSlot::Diagnostics)
        .unwrap()
        .owner()
        .clone();
    let query_owner = project
        .pool()
        .acquire(tsr_project::CheckerSlot::Query(0))
        .unwrap()
        .owner()
        .clone();
    let supplied =
        tsr_compiler::CheckedProgram::with_pool(program.clone(), Arc::new(project.clone()));
    let diagnostics = CheckerRequest {
        lifetime: tsr_checker::CheckerLifetime::Diagnostics,
        cancellation: Some(canceled),
    };
    assert_eq!(
        supplied
            .semantic_diagnostics(&diagnostics, Some(file))
            .unwrap(),
        vec![]
    );
    assert!(diagnostics_owner.was_canceled());
    assert!(!query_owner.was_canceled());
    let fresh = CheckerRequest {
        lifetime: tsr_checker::CheckerLifetime::Diagnostics,
        cancellation: None,
    };
    let codes: Vec<i32> = supplied
        .semantic_diagnostics(&fresh, Some(file))
        .unwrap()
        .iter()
        .map(|d| d.code)
        .collect();
    assert_eq!(codes, vec![2322, 2322, 2322]);
    let served = project
        .pool()
        .acquire(tsr_project::CheckerSlot::Diagnostics)
        .unwrap()
        .owner()
        .clone();
    assert!(
        !Arc::ptr_eq(&served, &diagnostics_owner),
        "the canceled diagnostics checker was disposed"
    );
}

/// Contract 5, cancellation during deferred nodes and at the unused-identifier
/// passes (checker.go `checkDeferredNodes`, `checkBlock` through
/// `checkSourceElements`, `checkSourceFile`): canceled as its first deferred
/// function body starts, a check stops that body at its first statement poll
/// and checks no further deferred node; canceled as its last body starts, it
/// has recorded the errors of the earlier bodies and skips the
/// renamed-binding and unused-identifier passes, which the pin polls before. Either way the check
/// returns no diagnostics and leaves the checker canceled, and a type retained
/// before the cancellation stays readable.
#[test]
fn cancellation_in_deferred_nodes_skips_the_rest_and_keeps_retained_results() {
    let text: &[u8] = b"const f = () => { let x: number = \"a\"; };\nconst g = () => { let y: number = \"b\"; };\nconst h = () => { let z: number = \"c\"; };\ntype F = ({ a: string }: { a: number }) => void;\n";
    let options = || CompilerOptions {
        no_unused_locals: Tristate::TRUE,
        ..no_lib()
    };
    let codes = |diagnostics: Vec<tsr_ast::Diagnostic>| {
        let mut codes: Vec<i32> = diagnostics.iter().map(|d| d.code).collect();
        codes.sort_unstable();
        codes
    };

    // Uncanceled, the program reports three assignment errors, the renamed
    // binding in a type and three unused locals (the pinned tsgo reports the
    // same seven with a `lib.d.ts` that declares the global interfaces).
    let (plain, counters) = program(&[(b"/a.ts", text)], options());
    let source = plain.file(b"/a.ts").unwrap().source();
    let owner = checker(&plain, &counters, None);
    let found = owner
        .operation()
        .unwrap()
        .semantic_diagnostics(source)
        .unwrap();
    assert_eq!(codes(found), vec![2322, 2322, 2322, 2842, 6133, 6133, 6133]);

    for (at, recorded) in [(1, vec![]), (3, vec![2322, 2322])] {
        let (canceled, counters) = program(&[(b"/a.ts", text)], options());
        let source = canceled.file(b"/a.ts").unwrap().source();
        let token = tsr_core::CancellationToken::new();
        let sink = CancelAt::new("checkDeferredNode", at, &token);
        let owner = checker(
            &canceled,
            &counters,
            Some(Tracer::new(sink.clone() as Arc<dyn TraceSink>, 0)),
        );
        let mut op = owner.operation().unwrap();
        let number = op.builtin_type("numberType").unwrap();
        let retained = op.retain_type(number).unwrap();
        assert_eq!(
            op.semantic_diagnostics_cancellable(source, &token).unwrap(),
            vec![]
        );
        assert_eq!(
            sink.seen(),
            at,
            "no deferred body after the canceled one is checked"
        );
        assert_eq!(
            codes(op.recorded_diagnostics_probe(source).unwrap()),
            recorded,
            "canceled at deferred node {at}"
        );
        assert!(op.was_canceled() && owner.was_canceled());
        assert_eq!(
            op.semantic_diagnostics(source).unwrap_err(),
            tsr_checker::Error::PreviouslyCanceled
        );
        let restored = op.import_type(&retained).unwrap();
        assert_eq!(
            op.type_to_string(restored, 0).unwrap().as_bytes(),
            b"number"
        );
    }
}

const ASSIGNMENTS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../data/phase2/c6-assignments.json"
);

fn integers(value: &Value) -> Vec<i64> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|number| number.as_i64().unwrap())
        .collect()
}

/// `count` script files with the given compiler options, the program's own
/// single-threaded setting and no default library.
fn files_program(
    count: usize,
    options: CompilerOptions,
    single_threaded: Tristate,
) -> (Arc<Program>, Counters) {
    let names: Vec<Vec<u8>> = (0..count)
        .map(|i| format!("/f{i}.ts").into_bytes())
        .collect();
    let texts: Vec<Vec<u8>> = (0..count)
        .map(|i| {
            // A chain of imports gives the partition an import graph.
            let next = (i + 1) % count;
            format!("import {{ v{next} }} from \"./f{next}\";\nexport const v{i}: number = 1;\n")
                .into_bytes()
        })
        .collect();
    let files: Vec<(&[u8], &[u8])> = names
        .iter()
        .zip(&texts)
        .map(|(name, text)| (name.as_slice(), text.as_slice()))
        .collect();
    program_in(&files, options, single_threaded)
}

/// Contract 1, partitioning (compiler/checkerpool.go
/// `getCheckerAssociationPolicy`, `shouldPrioritizeSourceFiles`,
/// `getCheckerAssociationBaseWeight`, `getCheckerAssociationWeights`,
/// `getCheckerAssociationOrder`, `getCheckerAssociationsInOrder`,
/// `newCheckerPoolWithTracing`): over the recorded synthetic graphs at 2, 4
/// and 8 checkers (`data/phase2/c6-assignments.json`), where score ties, the
/// least-loaded fallback, the one-percent slack, each regime, import
/// normalization and its clamp each decide an assignment, the regime,
/// weights, stream order and associations equal the pin's. The two cases
/// that arm64's fused score arithmetic decides are checked on the
/// architecture the record was taken on, where the pin's result is theirs.
/// The checker count is the pin's: four, one when the program is
/// single-threaded (its own setting before the compiler option's), otherwise
/// the `checkers` option, clamped to the file count and 256 and to at least
/// one.
#[test]
fn partitioning_equals_the_pins_associations_and_count_rule() {
    let record: Value = serde_json::from_slice(&std::fs::read(ASSIGNMENTS).unwrap()).unwrap();
    let host = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else if cfg!(target_arch = "x86_64") {
        "amd64"
    } else {
        "other"
    };
    let mut decided = BTreeMap::new();
    for case in record["synthetic"].as_array().unwrap() {
        let features: Vec<&str> = case["features"]
            .as_array()
            .unwrap()
            .iter()
            .map(|feature| feature.as_str().unwrap())
            .collect();
        if features.contains(&"fusion") && record["goarch"] != host {
            continue;
        }
        let checker_count = usize::try_from(case["checker_count"].as_u64().unwrap()).unwrap();
        let adjacency = case["adjacency"]
            .as_array()
            .unwrap()
            .iter()
            .map(|adjacent| {
                integers(adjacent)
                    .into_iter()
                    .map(|index| usize::try_from(index).unwrap())
                    .collect()
            })
            .collect();
        let plan = CheckerAssociationPlan::compute(
            checker_count,
            integers(&case["node_counts"]),
            integers(&case["text_lengths"]),
            integers(&case["import_counts"]),
            case["is_declaration_file"]
                .as_array()
                .unwrap()
                .iter()
                .map(|flag| flag.as_bool().unwrap())
                .collect(),
            adjacency,
        );
        let policy = plan.policy.unwrap();
        let observed = json!({
            "policy": {
                "prioritize_source_files": policy.prioritize_source_files,
                "source_file_weight_multiplier": policy.source_file_weight_multiplier,
                "balance_penalty_multiplier": policy.balance_penalty_multiplier,
            },
            "file_weights": plan.file_weights,
            "order": plan.order,
            "associations": plan.associations,
        });
        assert_eq!(observed, case["native"], "{}", case["id"]);
        for feature in features {
            *decided.entry((checker_count, feature)).or_insert(0) += 1;
        }
    }
    for checker_count in [2, 4, 8] {
        for feature in [
            "tie",
            "fallback",
            "slack",
            "source_dominated",
            "imports",
            "import_unit_clamp",
        ] {
            assert!(
                decided.contains_key(&(checker_count, feature)),
                "no recorded {feature} case at {checker_count} checkers"
            );
        }
    }
    assert!(decided.contains_key(&(2, "declaration_heavy_small")));
    assert!(decided.contains_key(&(4, "declaration_heavy_strong")));
    assert!(decided.contains_key(&(8, "declaration_heavy_strong")));

    let count = |files: usize, checkers: Option<isize>, option: Tristate, own: Tristate| {
        let options = CompilerOptions {
            checkers,
            single_threaded: option,
            ..no_lib()
        };
        let (program, counters) = files_program(files, options, own);
        CompilerCheckerPool::new(program, &counters).checker_count()
    };
    let unknown = Tristate::UNKNOWN;
    assert_eq!(count(1, None, unknown, unknown), 1);
    assert_eq!(count(3, None, unknown, unknown), 3);
    assert_eq!(count(6, None, unknown, unknown), 4);
    assert_eq!(count(6, Some(2), unknown, unknown), 2);
    assert_eq!(count(6, Some(8), unknown, unknown), 6);
    assert_eq!(count(6, Some(0), unknown, unknown), 1);
    assert_eq!(count(6, Some(-3), unknown, unknown), 1);
    assert_eq!(count(300, Some(1000), unknown, unknown), 256);
    assert_eq!(count(6, Some(8), Tristate::TRUE, unknown), 1);
    assert_eq!(count(6, None, Tristate::TRUE, Tristate::FALSE), 4);
    assert_eq!(count(6, Some(3), unknown, Tristate::TRUE), 1);
}

/// Contract 2, acquisition (compiler/checkerpool.go `GetChecker`,
/// `getCheckerForFileExclusive`, `getCheckerForFileNonExclusive`,
/// `getCheckerNonExclusive`, `forEachCheckerGroupDo`): a file's checker is
/// the same on every call and is the one the association plan names; an
/// exclusive acquisition holds its checker until it is released, so another
/// thread's acquisition of that checker, exclusive or through the lock-free
/// hand-out's per-call operation (a resolver call), waits for the release; a
/// file outside the program matches no checker;
/// the interface's `GetChecker` serves the file's checker and, without a
/// file, the first; and one task per checker, each on its own thread,
/// visits exactly that checker's files, in program order.
#[test]
fn acquisition_is_exclusive_per_checker_and_affinity_is_stable() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Mutex};
    use tsr_checker::{CheckerLifetime, CheckerPool};

    let (program, counters) = files_program(8, no_lib(), Tristate::FALSE);
    let pool = CompilerCheckerPool::new(program.clone(), &counters);
    assert_eq!(pool.checker_count(), 4);
    let sources: Vec<NodeId> = program.files().iter().map(|file| file.source()).collect();
    let plan = pool.association_plan().unwrap().clone();
    let checkers = pool.checkers().unwrap();
    for (index, &source) in sources.iter().enumerate() {
        let owner = pool.checker_for_file_non_exclusive(source).unwrap();
        assert!(Arc::ptr_eq(owner, &checkers[plan.associations[index]]));
        assert!(Arc::ptr_eq(
            owner,
            pool.checker_for_file_non_exclusive(source).unwrap()
        ));
        let operation = pool.checker_for_file_exclusive(source).unwrap();
        assert!(Arc::ptr_eq(operation.owner(), owner));
        drop(operation);
        let mut served = None;
        pool.with_checker(CheckerLifetime::Temporary, Some(source), &mut |operation| {
            served = Some(operation.owner().clone());
            Ok(())
        })
        .unwrap();
        assert!(Arc::ptr_eq(&served.unwrap(), owner));
    }
    let mut first = None;
    pool.with_checker(CheckerLifetime::Temporary, None, &mut |operation| {
        first = Some(operation.owner().clone());
        Ok(())
    })
    .unwrap();
    assert!(Arc::ptr_eq(&first.unwrap(), &checkers[0]));
    assert!(Arc::ptr_eq(
        pool.checker_non_exclusive().unwrap(),
        &checkers[0]
    ));

    let source = sources[0];
    for per_call in [false, true] {
        let released = AtomicBool::new(false);
        let (held, wait) = mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let operation = pool.checker_for_file_exclusive(source).unwrap();
                held.send(()).unwrap();
                std::thread::sleep(std::time::Duration::from_millis(200));
                released.store(true, Ordering::SeqCst);
                drop(operation);
            });
            wait.recv().unwrap();
            let operation = if per_call {
                pool.checker_for_file_non_exclusive(source)
                    .unwrap()
                    .operation()
                    .unwrap()
            } else {
                pool.checker_for_file_exclusive(source).unwrap()
            };
            assert!(
                released.load(Ordering::SeqCst),
                "the second acquisition waited for the release"
            );
            drop(operation);
        });
    }

    let visits = Mutex::new(Vec::new());
    pool.for_each_checker_group_do(&sources, false, &|operation, position, file| {
        let checker = checkers
            .iter()
            .position(|owner| Arc::ptr_eq(owner, operation.owner()))
            .unwrap();
        visits
            .lock()
            .unwrap()
            .push((checker, std::thread::current().id(), position, file));
    })
    .unwrap();
    let visits = visits.into_inner().unwrap();
    assert_eq!(visits.len(), sources.len());
    let mut threads = std::collections::BTreeSet::new();
    for checker in 0..checkers.len() {
        let mine: Vec<_> = visits.iter().filter(|visit| visit.0 == checker).collect();
        let expected: Vec<usize> = (0..sources.len())
            .filter(|&position| plan.associations[position] == checker)
            .collect();
        assert_eq!(
            mine.iter().map(|visit| visit.2).collect::<Vec<_>>(),
            expected
        );
        assert!(mine.iter().all(|visit| visit.3 == sources[visit.2]));
        let thread = mine.first().map(|visit| visit.1);
        assert!(
            mine.iter().all(|visit| Some(visit.1) == thread),
            "one task per checker"
        );
        if let Some(thread) = thread {
            assert_ne!(thread, std::thread::current().id());
            threads.insert(format!("{thread:?}"));
        }
    }
    assert_eq!(
        threads.len(),
        plan.associations
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    );

    // A file outside the program matches no checker and is skipped, as the
    // pin's nil association is.
    let (other, _) = files_program(1, no_lib(), Tristate::FALSE);
    let mut with_foreign = sources.clone();
    with_foreign.insert(1, other.files()[0].source());
    let visited = Mutex::new(0);
    pool.for_each_checker_group_do(&with_foreign, false, &|_, position, _| {
        assert_ne!(position, 1);
        *visited.lock().unwrap() += 1;
    })
    .unwrap();
    assert_eq!(visited.into_inner().unwrap(), sources.len());
}

/// Contract 3, work groups (core/workgroup.go `NewWorkGroup`, the
/// single-threaded group's `Queue`, `RunAndWait` and `pop`, the parallel
/// group's; compiler/checkerpool.go `forEachCheckerGroupDo`,
/// `forEachCheckerParallel`, `GetGlobalDiagnostics`): a single-threaded group
/// runs the checkers' tasks on the caller's thread, the last queued first,
/// which is the order the pin's shows wherever task order is observable; a
/// parallel group runs every checker's task on its own thread; and the
/// pool's global diagnostics, every checker's concatenated, sorted and
/// deduplicated, equal one checker's.
#[test]
fn work_groups_order_tasks_as_the_pin_and_global_diagnostics_merge() {
    use std::sync::Mutex;

    let (program, counters) = files_program(8, no_lib(), Tristate::FALSE);
    let pool = CompilerCheckerPool::new(program.clone(), &counters);
    let sources: Vec<NodeId> = program.files().iter().map(|file| file.source()).collect();
    let checkers = pool.checkers().unwrap();
    let plan = pool.association_plan().unwrap().clone();
    let used: std::collections::BTreeSet<usize> = plan.associations.iter().copied().collect();
    assert!(
        used.len() > 1,
        "the witness spreads its files over several checkers"
    );

    let caller = std::thread::current().id();
    let order = Mutex::new(Vec::new());
    pool.for_each_checker_group_do(&sources, true, &|operation, _, _| {
        let checker = checkers
            .iter()
            .position(|owner| Arc::ptr_eq(owner, operation.owner()))
            .unwrap();
        assert_eq!(std::thread::current().id(), caller);
        let mut order = order.lock().unwrap();
        if order.last() != Some(&checker) {
            order.push(checker);
        }
    })
    .unwrap();
    let mut expected: Vec<usize> = used.iter().copied().collect();
    expected.reverse();
    assert_eq!(order.into_inner().unwrap(), expected, "last queued first");

    let threads = Mutex::new(Vec::new());
    pool.for_each_checker_parallel(&|index, operation| {
        assert!(Arc::ptr_eq(operation.owner(), &checkers[index]));
        threads.lock().unwrap().push(std::thread::current().id());
    })
    .unwrap();
    let threads = threads.into_inner().unwrap();
    assert_eq!(threads.len(), checkers.len());
    assert!(threads.iter().all(|thread| *thread != caller));
    assert_eq!(
        threads
            .iter()
            .map(|thread| format!("{thread:?}"))
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        checkers.len()
    );

    let merged = pool.global_diagnostics().unwrap();
    assert!(
        !merged.is_empty(),
        "a no-lib program reports its missing global types"
    );
    let single = checker(&program, &counters, None)
        .operation()
        .unwrap()
        .global_diagnostics()
        .unwrap();
    assert_eq!(
        merged,
        program.sort_and_deduplicate_diagnostics(&single).unwrap()
    );
}

const DEEP_RELATIONS: &str = include_str!("fixtures/c1/recursion.ts");
const DEEP_FLOW: &str = include_str!(
    "../../../upstream/tsc/testdata/tests/cases/compiler/binderBinaryExpressionStress.ts"
);

/// The deep-relation fixture beside five small files, in the concurrent mode.
fn deep_program(deep: &str) -> (Arc<Program>, Counters) {
    let others: Vec<(Vec<u8>, Vec<u8>)> = (0..5)
        .map(|i| {
            (
                format!("/f{i}.ts").into_bytes(),
                format!("export const v{i}: number = \"{i}\";\n").into_bytes(),
            )
        })
        .collect();
    let mut files: Vec<(&[u8], &[u8])> = vec![(b"/deep.ts", deep.as_bytes())];
    files.extend(
        others
            .iter()
            .map(|(name, text)| (name.as_slice(), text.as_slice())),
    );
    program_in(&files, no_lib(), Tristate::FALSE)
}

/// Contract 6, panic retirement in a multi-checker pool (ADR 0012; the pin
/// has no recovery, a Go panic ends the process): a panic inside one
/// checker's recursive relation, on a pool thread, while the program's
/// semantic diagnostics are collected (program.go `GetSemanticDiagnostics`,
/// checkerpool.go `forEachCheckerGroupDo`) retires the pool's generation. The
/// panic reaches the caller, the program publishes no diagnostics, and the
/// program's pool refuses further work, while a fresh program pool checks the
/// same program with the results of a pool that never panicked. A canceled
/// checker, by contrast, is poisoned but leaves its pool's generation live.
/// The per-file generation gates and the E3 scenarios over the compiler pool
/// are `checker_pool::ownership` in the crate's own tests.
#[test]
fn a_panic_in_one_pool_checker_retires_the_pool_and_a_fresh_pool_succeeds() {
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use tsr_compiler::CheckedProgram;

    let (program, counters) = deep_program(DEEP_RELATIONS);
    let reference = CheckedProgram::new(program.clone(), &counters, None)
        .semantic_diagnostics(&CheckerRequest::default(), None)
        .unwrap();
    let checked = CheckedProgram::new(program.clone(), &counters, None);
    let pool = checked.compiler_checker_pool().unwrap();
    assert_eq!(pool.checker_count(), 4);
    let deep = program.file(b"/deep.ts").unwrap().source();
    pool.checker_for_file_exclusive(deep)
        .unwrap()
        .begin_recursion_probe(Some(8))
        .unwrap();
    let panic = catch_unwind(AssertUnwindSafe(|| {
        checked.semantic_diagnostics(&CheckerRequest::default(), None)
    }))
    .expect_err("the relation reaches the injected panic");
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .unwrap_or("");
    assert!(
        message.starts_with("injected panic inside recursive relation at depth "),
        "{message}"
    );
    assert_eq!(pool.generation().validate(), Err(tsr_arena::Error::Retired));
    for owner in pool.checkers().unwrap() {
        assert!(matches!(
            owner.operation(),
            Err(tsr_checker::Error::Arena(tsr_arena::Error::Retired))
        ));
    }
    assert!(matches!(
        checked.semantic_diagnostics(&CheckerRequest::default(), None),
        Err(tsr_compiler::Error::Checker(tsr_checker::Error::Arena(
            tsr_arena::Error::Retired
        )))
    ));
    assert!(checked.global_diagnostics().is_err());

    let fresh = CheckedProgram::new(program.clone(), &counters, None);
    assert_eq!(
        fresh
            .semantic_diagnostics(&CheckerRequest::default(), None)
            .unwrap(),
        reference
    );

    let canceled = CheckedProgram::new(program.clone(), &counters, None);
    let token = tsr_core::CancellationToken::new();
    token.cancel();
    let target = program.file(b"/f0.ts").unwrap().source();
    canceled
        .with_type_checker_for_file_exclusive(
            &CheckerRequest::default(),
            target,
            &mut |operation| {
                assert_eq!(
                    operation.semantic_diagnostics_cancellable(target, &token)?,
                    vec![]
                );
                assert!(operation.was_canceled());
                Ok(())
            },
        )
        .unwrap();
    let pool = canceled.compiler_checker_pool().unwrap();
    assert!(pool.generation().validate().is_ok());
    let plan = pool.association_plan().unwrap().clone();
    let target_checker = plan.associations[program
        .files()
        .iter()
        .position(|f| f.source() == target)
        .unwrap()];
    for (file, &checker) in program.files().iter().zip(&plan.associations) {
        if checker != target_checker {
            assert!(canceled
                .semantic_diagnostics(&CheckerRequest::default(), Some(file))
                .is_ok());
        }
    }
}

/// Contract 9, stacks and lifecycle (ADR 0011; checkerpool.go
/// `createCheckers`, `forEachCheckerGroupDo`): pool checkers run on threads
/// with the reserved stacks, so the deep C1 relation and the deep C3 flow
/// graph complete on a pool thread with the results they have on the
/// caller's; the pool creates its checkers once per program and releases
/// them with it; and handles of one checker of the pool are rejected by
/// another.
#[test]
fn pool_threads_carry_reserved_stacks_and_the_pool_lives_with_its_program() {
    use std::sync::Mutex;
    use tsr_compiler::CheckedProgram;

    let (program, counters) = deep_program(DEEP_RELATIONS);
    let checked = CheckedProgram::new(program.clone(), &counters, None);
    let pool = checked.compiler_checker_pool().unwrap();
    let deep = program.file(b"/deep.ts").unwrap().source();
    let (a, b) = {
        let view = program.file(b"/deep.ts").unwrap().bound().view().ast();
        let statements: Vec<NodeId> = view
            .node_slice(view.node(deep).unwrap().statements(view).unwrap())
            .unwrap()
            .iter()
            .flatten()
            .collect();
        let name = |statement: NodeId| {
            let list = view
                .node(statement)
                .unwrap()
                .data_source()
                .as_variable_statement()
                .unwrap()
                .declaration_list()
                .unwrap();
            let declarations = view
                .node(list)
                .unwrap()
                .data_source()
                .as_variable_declaration_list()
                .unwrap()
                .declarations()
                .unwrap();
            let declaration = view
                .node_slice(view.list(declarations).unwrap().nodes())
                .unwrap()
                .get(0)
                .unwrap()
                .unwrap();
            view.node(declaration).unwrap().name().unwrap()
        };
        (
            name(statements[statements.len() - 4]),
            name(statements[statements.len() - 3]),
        )
    };
    let caller = std::thread::current().id();
    let observed = Mutex::new(None);
    pool.for_each_checker_group_do(&[deep], false, &|operation, _, _| {
        assert_ne!(std::thread::current().id(), caller);
        let left = operation.get_type_at_location(a).unwrap();
        let right = operation.get_type_at_location(b).unwrap();
        operation.begin_recursion_probe(None).unwrap();
        let related = operation
            .is_type_related_to(left, right, tsr_checker::RelationKind::Assignable)
            .unwrap();
        *observed.lock().unwrap() = Some((related, operation.take_recursion_probe().unwrap()));
    })
    .unwrap();
    let (related, probe) = observed.into_inner().unwrap().unwrap();
    assert!(
        related,
        "the depth limit ends the relation as on the caller's stack"
    );
    let remaining = probe["maximum_remaining_stack"].as_u64().unwrap();
    assert!(
        remaining > (tsr_core::workgroup::RESERVED_STACK / 2) as u64,
        "a pool thread's reserved stack: {probe}"
    );

    let (flow, flow_counters) = deep_program(DEEP_FLOW);
    let deep_flow = flow.file(b"/deep.ts").unwrap().source();
    let reference = CheckedProgram::new(flow.clone(), &flow_counters, None)
        .semantic_diagnostics(
            &CheckerRequest::default(),
            Some(flow.file(b"/deep.ts").unwrap()),
        )
        .unwrap();
    let flow_checked = CheckedProgram::new(flow.clone(), &flow_counters, None);
    let observed = Mutex::new(None);
    flow_checked
        .compiler_checker_pool()
        .unwrap()
        .for_each_checker_group_do(&[deep_flow], false, &|operation, _, _| {
            assert_ne!(std::thread::current().id(), caller);
            operation.begin_recursion_probe(None).unwrap();
            let diagnostics = operation.semantic_diagnostics(deep_flow).unwrap();
            *observed.lock().unwrap() =
                Some((diagnostics, operation.take_recursion_probe().unwrap()));
        })
        .unwrap();
    let (diagnostics, probe) = observed.into_inner().unwrap().unwrap();
    assert_eq!(
        flow.sort_and_deduplicate_diagnostics(&diagnostics).unwrap(),
        reference
    );
    let operators = DEEP_FLOW
        .lines()
        .map(|line| line.matches(" + ").count())
        .max()
        .unwrap();
    assert!(operators > 1000);
    assert!(
        probe["maximum_expression_depth"].as_u64().unwrap() >= operators as u64,
        "{probe}"
    );
    assert!(
        probe["maximum_remaining_stack"].as_u64().unwrap()
            > (tsr_core::workgroup::RESERVED_STACK / 2) as u64,
        "a pool thread's reserved stack: {probe}"
    );

    // Created once, released with the program's pool.
    let first: Vec<_> = pool.checkers().unwrap().iter().map(Arc::as_ptr).collect();
    checked
        .semantic_diagnostics(&CheckerRequest::default(), None)
        .unwrap();
    checked
        .semantic_diagnostics(&CheckerRequest::default(), None)
        .unwrap();
    let again: Vec<_> = pool.checkers().unwrap().iter().map(Arc::as_ptr).collect();
    assert_eq!(first, again);
    let weak: Vec<_> = pool
        .checkers()
        .unwrap()
        .iter()
        .map(Arc::downgrade)
        .collect();

    // A handle of one checker is rejected by another of the same pool.
    let other = program
        .files()
        .iter()
        .zip(&pool.association_plan().unwrap().associations)
        .find(|&(_, &checker)| checker != pool.association_plan().unwrap().associations[0])
        .map(|(file, _)| file.source())
        .unwrap();
    let first_file = program.files()[0].source();
    let (retained, id) = {
        let mut operation = pool.checker_for_file_exclusive(first_file).unwrap();
        let literal = operation.string_literal_type(b"one checker's").unwrap();
        (operation.retain_type(literal).unwrap(), literal)
    };
    {
        let operation = pool.checker_for_file_exclusive(other).unwrap();
        assert_eq!(
            operation.import_type(&retained),
            Err(tsr_checker::Error::Arena(tsr_arena::Error::WrongOwner))
        );
        assert_eq!(
            operation.type_flags(id),
            Err(tsr_checker::Error::Arena(tsr_arena::Error::WrongOwner))
        );
    }
    drop(retained);
    drop(checked);
    assert!(
        weak.iter().all(|owner| owner.upgrade().is_none()),
        "released with the program's pool"
    );
}

/// Union ordering over every checker, recorded at the pooled checkpoint.
struct UnionOrdering(Option<Value>);
impl executor::Hooks for UnionOrdering {
    fn checkpoint_checkers(&mut self, checkers: &mut tsr_compiler::FileCheckers<'_, '_>) {
        let operations: Vec<_> = checkers.iter().collect();
        self.0 = Some(subtests::union_ordering_checkers(&operations));
    }
}

/// Contract 8, two modes (the pin's harness with
/// `TS_TEST_PROGRAM_SINGLE_THREADED` true and false; program.go
/// `collectCheckerDiagnostics`, `collectDiagnosticsFromFiles`,
/// `GetTypeCheckerForFile`; compiler_runner.go `verifyUnionOrdering`): a
/// multi-file corpus program with cross-file types, checked through its pool
/// with one checker single-threaded and with four checkers concurrently,
/// equals in each mode that mode's native observation for errors, types,
/// symbols, display and union ordering. The pin's modes differ in union
/// ordering's counts (73 unions on one checker, 94 over four, where only the
/// checkers of the importing files create their unions), and the Rust modes
/// differ the same way. The fixture is `fixtures/c6/modes`, frozen from both
/// verified native captures by its `regenerate.py`.
#[test]
fn both_modes_match_their_native_observations() {
    use sha2::{Digest, Sha256};

    let raw = include_str!("fixtures/c6/modes/request.json");
    let native: Value =
        serde_json::from_str(include_str!("fixtures/c6/modes/native.json")).unwrap();
    let pin: Value = serde_json::from_str(include_str!("../../../data/upstream.json")).unwrap();
    assert_eq!(native["pin"], pin["pin"]);
    assert_eq!(
        native["request_sha256"],
        format!("{:x}", Sha256::digest(raw.as_bytes()))
    );
    let request: Value = serde_json::from_str(raw).unwrap();
    assert_eq!(request["id"], native["id"]);
    assert_ne!(
        native["modes"]["single"]["union_ordering"],
        native["modes"]["concurrent"]["union_ordering"]
    );
    for (mode, checkers) in [("single", 1), ("concurrent", 4)] {
        let mut moded = request.clone();
        moded["mode"] = json!(mode);
        let mut hooks = UnionOrdering(None);
        let actual = corpus::observe_with(&moded, &mut FileCache::new(), &mut hooks, false);
        let expected = &native["modes"][mode];
        assert_eq!(actual["mode"], json!(mode));
        assert_eq!(actual["checker_count"], json!(checkers), "{mode}");
        assert_eq!(
            actual["error_baseline"]["state"], "executed",
            "{mode}: {actual}"
        );
        assert_eq!(
            actual["error_baseline"]["baseline"], expected["errors"],
            "{mode} errors"
        );
        for domain in ["types", "symbols", "public_type_strings"] {
            assert_eq!(
                actual["type_symbol_baselines"][domain], expected[domain],
                "{mode} {domain}"
            );
        }
        assert_eq!(
            hooks.0.unwrap(),
            expected["union_ordering"],
            "{mode} union ordering"
        );
    }
}
