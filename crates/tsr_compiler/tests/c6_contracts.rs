//! Phase 2 C6 direct contracts (docs/PHASE2-C6-plan.md, C6.9), each over
//! production entry points with its pinned Go counterpart named.
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use tsr_arena::{CheckerIdentity, Counters, Generation, NodeId};
use tsr_checker::{
    CheckerOwner, MemoryTraceSink, TraceLocation, TraceSink, TraceTypeRecord, TraceValue, Tracer,
};
use tsr_compiler::{FileCache, Program, ProgramCheckerHost, ProgramOptions};
use tsr_core::{CompilerOptions, Tristate};
use tsr_jsstring::JsString;

const TRACE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/c6/trace");

fn program(files: &[(&[u8], &[u8])], options: CompilerOptions) -> (Arc<Program>, Counters) {
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
/// it at release and serves a fresh checker that checks the file.
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
