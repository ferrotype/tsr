//! Actual loader operations feed the Phase 4 tracing seam, including sampled
//! operations that deterministic corpus runs intentionally omit.
use std::sync::Arc;
use tsr_arena::Counters;
use tsr_checker::{MemoryTraceSink, TracePhase, TraceValue};
use tsr_compiler::{FileCache, Program, ProgramOptions};
use tsr_core::{CompilerOptions, Tristate};
use tsr_jsstring::JsString;

fn js(value: &str) -> JsString {
    JsString::from_bytes(value.as_bytes())
}

#[test]
fn loader_sampled_spans_cover_references_resolution_and_missing_types() {
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/src", true);
    fs.insert_loaded(
        b"/src/main.ts",
        b"/// <reference types=\"missing\" />\nimport { x } from './dep'; export { x };\n"
            .as_slice(),
    );
    fs.insert_loaded(b"/src/dep.ts", b"export const x = 1;\n".as_slice());
    fs.insert_loaded(
        b"/src/node_modules/@types/ambient/index.d.ts",
        b"declare const ambient: number;\n".as_slice(),
    );
    fs.insert_loaded(
        b"/src/ref/tsconfig.json",
        br#"{"compilerOptions":{"composite":true},"files":["ref.ts"]}"#.as_slice(),
    );
    fs.insert_loaded(b"/src/ref/ref.ts", b"export {};\n".as_slice());
    fs.insert_loaded(b"/lib/lib.es5.d.ts", b"interface Object {}\n".as_slice());
    let mut config = tsr_tsoptions::ParsedCommandLine::new(
        CompilerOptions {
            types: Some(vec![js("ambient")]),
            lib_replacement: Tristate::TRUE,
            lib: Some(vec![js("es5")]),
            ..Default::default()
        },
        vec![js("/src/main.ts")],
    );
    config.project_references = Some(vec![tsr_tsoptions::ProjectReference {
        path: js("/src/ref"),
        original_path: js("./ref"),
        circular: false,
    }]);
    let trace = Arc::new(MemoryTraceSink::new(false));
    let program = Program::load_live_with_content_mapper_project_and_tracing(
        ProgramOptions {
            config,
            host: Arc::new(fs.finish()),
            current_directory: js("/src"),
            default_library_path: js("/lib"),
            skip_module_resolution: false,
            single_threaded: Tristate::TRUE,
        },
        None,
        &mut FileCache::new(),
        &Counters::new(),
        Some(trace.clone()),
    )
    .unwrap();
    assert!(program.source_file(b"/src/dep.ts").is_some());
    let events = trace.events();
    assert_eq!(
        (events[0].ph, events[0].name.as_str()),
        ("B", "createProgram")
    );
    assert_eq!(
        (
            events.last().unwrap().ph,
            events.last().unwrap().name.as_str()
        ),
        ("E", "createProgram")
    );
    let sampled = |name: &'static str| {
        events
            .iter()
            .filter(move |event| event.ph == "X" && event.name == name)
    };
    for name in [
        "processRootFiles",
        "processTypeReferences",
        "findSourceFile",
        "resolveModuleNamesWorker",
        "resolveLibrary",
        "resolveTypeReferenceDirectiveNamesWorker",
    ] {
        assert!(
            sampled(name).any(|event| event.phase == TracePhase::Program),
            "missing sampled span: {name}"
        );
    }
    let reference = sampled("parseJsonSourceFileConfigFileContent")
        .next()
        .unwrap();
    assert_eq!(reference.phase, TracePhase::Parse);
    assert_eq!(
        reference.args["path"],
        TraceValue::Str("/src/ref/tsconfig.json".into())
    );
    assert_eq!(
        sampled("processRootFiles").next().unwrap().args["count"],
        TraceValue::Int(1)
    );
    let directives: Vec<_> = sampled("processTypeReferenceDirective").collect();
    assert_eq!(directives.len(), 2);
    let automatic = directives
        .iter()
        .find(|event| event.args["refKind"] == TraceValue::Int(6))
        .unwrap();
    assert_eq!(
        automatic.args["directive"],
        TraceValue::Str("ambient".into())
    );
    assert_eq!(automatic.args["hasResolved"], TraceValue::Bool(true));
    assert!(!automatic.args.contains_key("refPath"));
    let explicit = directives
        .iter()
        .find(|event| event.args["refKind"] == TraceValue::Int(2))
        .unwrap();
    assert_eq!(
        explicit.args["directive"],
        TraceValue::Str("missing".into())
    );
    assert_eq!(explicit.args["hasResolved"], TraceValue::Bool(false));
    assert_eq!(
        explicit.args["refPath"],
        TraceValue::Str("/src/main.ts".into())
    );
}
