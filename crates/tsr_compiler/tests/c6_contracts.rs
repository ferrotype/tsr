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
