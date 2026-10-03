//! Phase 2 C7 content-mapper contracts (docs/PHASE2-C7-plan.md, C7.8.5), each
//! over production entry points with its pinned Go counterpart named.
//!
//! The rows are the 15 executed conformance rows that run content mappers,
//! observed through the corpus harness in both test-program modes. Each is
//! compared with the verified native captures (`fixtures/c7`, frozen from the
//! Phase 2 native capture) and with the content-mapper baseline the pin commits for
//! it (`compilerTest.verifyContentMapper`), which renders every mapped file's
//! diagnostics against the text its span maps to.
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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use tsr_compiler::FileCache;
use tsr_jsstring::JsString;

const REFERENCE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../upstream/tsc/testdata/baselines/reference/compiler"
);

struct Fixture {
    requests: BTreeMap<String, Value>,
    native: Value,
}

fn fixture() -> Fixture {
    use sha2::{Digest, Sha256};

    let raw = include_str!("fixtures/c7/requests.json");
    let native: Value = serde_json::from_str(include_str!("fixtures/c7/native.json")).unwrap();
    let pin: Value = serde_json::from_str(include_str!("../../../data/upstream.json")).unwrap();
    assert_eq!(native["pin"], pin["pin"]);
    assert_eq!(
        native["requests_sha256"],
        format!("{:x}", Sha256::digest(raw.as_bytes()))
    );
    let requests: Vec<Value> = serde_json::from_str(raw).unwrap();
    assert_eq!(requests.len(), 15);
    Fixture {
        requests: requests
            .into_iter()
            .map(|request| (request["id"].as_str().unwrap().to_owned(), request))
            .collect(),
        native,
    }
}

fn id(short: &str) -> String {
    format!("compiler/contentMapper{short}.ts#configuration=0")
}

fn unhex(value: &Value) -> Vec<u8> {
    let text = value.as_str().expect("hex text");
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap())
        .collect()
}

/// The row observed through the corpus harness in `mode`, with its error
/// domain checked against the native observation of that mode and its
/// content-mapper baseline against the pin's reference. Returns the row's
/// error baseline.
fn observe(fixture: &Fixture, short: &str, mode: &str) -> Value {
    let id = id(short);
    let mut request = fixture.requests[&id].clone();
    request["mode"] = json!(mode);
    let actual = corpus::observe_with(
        &request,
        &mut FileCache::new(),
        &mut executor::NoHooks,
        false,
    );
    assert_eq!(
        actual["checker_count"],
        json!(if mode == "single" { 1 } else { 4 }),
        "{id} {mode}"
    );
    let errors = actual["error_baseline"].clone();
    assert_eq!(errors["state"], "executed", "{id} {mode}: {errors}");
    let native = &fixture.native["rows"][&id][mode];
    for (field, expected) in [
        ("pre_diagnostics", "error_pre_diagnostics"),
        ("diagnostics", "error_post_diagnostics"),
        ("render_diagnostics", "error_diagnostics"),
        ("inputs", "error_render_inputs"),
        ("pretty", "error_pretty"),
        ("baseline", "errors"),
    ] {
        assert_eq!(errors[field], native[expected], "{id} {mode} {field}");
    }
    let reference =
        std::fs::read(format!("{REFERENCE}/contentMapper{short}.contentmapper")).unwrap();
    assert_eq!(
        String::from_utf8(unhex(&errors["content_mapper"]["text_hex"])).unwrap(),
        String::from_utf8(reference).unwrap(),
        "{id} {mode} content-mapper baseline"
    );
    errors
}

/// Observes `short` in both modes; the pin's modes agree on every error
/// observation of these rows.
fn observe_both(fixture: &Fixture, short: &str) -> Value {
    let single = observe(fixture, short, "single");
    let concurrent = observe(fixture, short, "concurrent");
    assert_eq!(single, concurrent, "{short}: the modes differ");
    single
}

fn codes(diagnostics: &Value) -> Vec<i64> {
    diagnostics
        .as_array()
        .unwrap()
        .iter()
        .map(|diagnostic| diagnostic["code"].as_i64().unwrap())
        .collect()
}

/// A mapped file is checked as its transformed text, and its diagnostics keep
/// their virtual spans; the diagnostic writer renders each against the text
/// its span maps to, and shows an aliased name in its original spelling while
/// the stored argument stays the virtual one. An unnecessary-code report on
/// synthesized text is dropped (`filterAndSortDiagnostics`).
/// Pin: fileloader.go:fileLoader.parseSourceFile, program.go:filterAndSortDiagnostics,
/// ast/diagnostic.go:Diagnostic.displayMessageArgs, diagnosticwriter.go:ASTDiagnostic.
#[test]
fn mapped_diagnostics_render_against_the_text_their_spans_map_to() {
    let fixture = fixture();
    let errors = observe_both(&fixture, "Diagnostics");
    // The checker's two assignments and the mapper's own report, at the
    // virtual offsets past the transform's 27-byte preamble.
    assert_eq!(codes(&errors["diagnostics"]), [2322, 2322, 1000]);
    assert_eq!(errors["diagnostics"][0]["pos"], 40);
    assert_eq!(errors["diagnostics"][2]["source_hex"], json!("626f78"));
    let alias = observe_both(&fixture, "AliasDiagnostic");
    // The stored argument stays the virtual name; the reference shows '+'.
    assert_eq!(
        alias["diagnostics"][0]["args_hex"],
        json!([errors::hex(b"add")])
    );
    let unused = observe_both(&fixture, "SynthesizedUnusedDiagnostics");
    assert_eq!(codes(&unused["diagnostics"]), [6133]);
    for short in [
        "Transform",
        "DeclarationEmit",
        "PerFileExtension",
        "Symlink",
    ] {
        let errors = observe_both(&fixture, short);
        assert_eq!(errors["diagnostics"], json!([]), "{short}");
    }
}

/// A file whose transform fails loads as an empty, still content-mapped stub,
/// with one diagnostic naming the failure: a failing mapper, an unsupported
/// virtual extension, a supplemental name that collides with a program file,
/// and each malformed diagnostic directive.
/// Pin: fileloader.go:fileLoader.parseSourceFile, fileLoader.emptyContentMappedFile,
/// contentMapperTransformDiagnostic.
#[test]
fn a_failed_transform_leaves_an_empty_mapped_stub_and_one_diagnostic() {
    let fixture = fixture();
    for (short, files) in [
        ("DeclarationEmitFailure", 1),
        ("InvalidExtension", 1),
        ("SupplementalFileCollision", 1),
        ("InvalidDiagnosticDirectives", 5),
    ] {
        let errors = observe_both(&fixture, short);
        assert_eq!(
            codes(&errors["diagnostics"]),
            vec![100_025; files],
            "{short}"
        );
        for diagnostic in errors["diagnostics"].as_array().unwrap() {
            assert_eq!(
                (&diagnostic["pos"], &diagnostic["end"]),
                (&json!(0), &json!(0))
            );
        }
    }
}

/// A mapper's supplemental files join the program with their own include
/// reason: their diagnostics render in the error baseline (they are not
/// content-mapped themselves), their globals are shared by the mapped files,
/// and their modules resolve as the mapped file imports them.
/// Pin: filesparser.go:parseTask.load, fileInclude.go:FileIncludeReason.computeDiagnostic.
#[test]
fn supplemental_files_carry_their_diagnostics_globals_and_modules() {
    let fixture = fixture();
    let diagnostics = observe_both(&fixture, "SupplementalDiagnostics");
    assert_eq!(codes(&diagnostics["render_diagnostics"]), [2304, 2322]);
    assert_eq!(diagnostics["baseline"]["state"], "content");
    let module = observe_both(&fixture, "SupplementalModule");
    assert_eq!(codes(&module["render_diagnostics"]), [2322]);
    let globals = observe_both(&fixture, "SupplementalGlobals");
    assert_eq!(globals["diagnostics"], json!([]));
}

/// Mapped diagnostic directives suppress the checker diagnostics their
/// virtual range covers, never a mapper's own or a syntax error, and an
/// expectation nothing met reports its unused diagnostic at its original
/// range.
/// Pin: program.go:applyContentMapperDiagnosticDirectives,
/// program.go:Program.getBindAndCheckDiagnosticsWithChecker.
#[test]
fn mapped_directives_suppress_and_report_as_the_pin() {
    let fixture = fixture();
    let errors = observe_both(&fixture, "DiagnosticDirectives");
    assert_eq!(
        codes(&errors["diagnostics"]),
        [2578, 2578, 1109, 2322, 2578, 1000]
    );
}

/// Counts the mapper processes a host starts and signals when each one's
/// connection ends. It serves the transforming mapper, as the pin's
/// `contentmappertest.Serve` does.
struct CountingSpawner {
    spawned: AtomicUsize,
    ended: Mutex<mpsc::Sender<()>>,
}

impl tsr_contentmapper::Spawner for CountingSpawner {
    fn spawn(
        &self,
        command: &[JsString],
        _dir: &[u8],
        _stderr: Box<dyn std::io::Write + Send>,
    ) -> Result<tsr_ipc::Stream, tsr_contentmapper::SpawnError> {
        assert_eq!(
            command.first().map(JsString::as_bytes),
            Some(tsr_contentmappertest::TRANSFORMING_MAPPER.as_bytes())
        );
        self.spawned.fetch_add(1, Ordering::SeqCst);
        let (client, server) = tsr_ipc::pipe();
        let ended = self.ended.lock().unwrap().clone();
        std::thread::spawn(move || {
            let _ = tsr_contentmappertest::serve(&tsr_ipc::Context::background(), server);
            let _ = ended.send(());
        });
        Ok(client)
    }
}

/// The harness opens one host and one project per compilation, which the
/// pre-emit and post-emit programs share: the mapper process starts once and
/// transforms for both, each program's diagnostics are the native ones, and
/// closing the project and then the host at the end of the row ends the
/// mapper's connection.
/// Pin: testutil/harnessutil/harnessutil.go:CompileFilesEx (the host and
/// project with their deferred closes), compiler/host.go:compilerHost.ContentMapperProject.
#[test]
fn one_host_serves_the_pre_and_post_emit_programs_and_closes_with_the_row() {
    let fixture = fixture();
    let id = id("Diagnostics");
    let request = &fixture.requests[&id];
    let native = &fixture.native["rows"][&id]["single"];
    let (ended, connection_ended) = mpsc::channel();
    let spawner = Arc::new(CountingSpawner {
        spawned: AtomicUsize::new(0),
        ended: Mutex::new(ended),
    });
    {
        let _scope = executor::content_mapper_scope(request, spawner.clone()).unwrap();
        assert!(executor::content_mapper_project().is_some());
        let mut cache = FileCache::new();
        for field in ["error_pre_diagnostics", "error_post_diagnostics"] {
            let fresh = executor::load_fresh(request, &mut cache).unwrap();
            let mut operation = fresh.owner.operation().unwrap();
            let values = executor::harness_diagnostics(
                &fresh.program,
                &mut operation,
                &request["diagnostic_phases"],
            )
            .unwrap();
            let sorted = fresh
                .program
                .sort_and_deduplicate_diagnostics(&values)
                .unwrap();
            assert_eq!(
                executor::diagnostics::phase(&fresh.program, &sorted)["diagnostics"],
                native[field],
                "{field}"
            );
        }
        assert_eq!(spawner.spawned.load(Ordering::SeqCst), 1);
        assert!(connection_ended.try_recv().is_err());
    }
    assert!(executor::content_mapper_project().is_none());
    connection_ended
        .recv_timeout(Duration::from_secs(10))
        .expect("closing the host ends the mapper's connection");
    assert_eq!(spawner.spawned.load(Ordering::SeqCst), 1);
}

/// A mapper process lives while a project leases it: a second project over
/// the same specification shares the lease and the process, releasing the
/// last lease closes the process's connection, a later project starts a new
/// one, and closing the host ends every connection still open and refuses
/// new projects.
/// Pin: contentmapper/hostimpl.go:host.Project, projectLease.release,
/// host.release, host.Close.
#[test]
fn a_mapper_process_lives_while_a_project_leases_it() {
    use tsr_contentmapper::{Host, ProjectSpec, Request};
    use tsr_tsoptions::config_mappers::{ContentMapper, MapperManifest};

    let text = |bytes: &[u8]| JsString::from_bytes(bytes);
    let spec = ProjectSpec {
        config_file_name: text(b"/tsconfig.json"),
        mappers: Arc::from(vec![ContentMapper {
            package: text(b"mapper"),
            extensions: vec![text(b".box")],
            manifest: MapperManifest {
                name: text(b"mapper"),
                version: text(b"1.0.0"),
                exec: Some(vec![text(
                    tsr_contentmappertest::TRANSFORMING_MAPPER.as_bytes(),
                )]),
                compiler_options: Some(
                    tsr_contentmappertest::DECLARED_OPTIONS
                        .iter()
                        .map(|name| text(name.as_bytes()))
                        .collect(),
                ),
                ..MapperManifest::default()
            },
            ..ContentMapper::default()
        }]),
        compiler_options: Arc::new(tsr_core::CompilerOptions::default()),
    };
    let request = Request {
        file_name: text(b"/widget.box"),
        content: b"export const count: number = 1;\n".to_vec(),
    };
    let (ended, connection_ended) = mpsc::channel();
    let spawner = Arc::new(CountingSpawner {
        spawned: AtomicUsize::new(0),
        ended: Mutex::new(ended),
    });
    let spawned = || spawner.spawned.load(Ordering::SeqCst);
    let host = tsr_contentmapper::new_host(
        &tsr_ipc::Context::background(),
        spawner.clone(),
        tsr_locale::Locale::default(),
    );
    let first = host.project(spec.clone()).unwrap();
    first.transform(0, &request).unwrap();
    let second = host.project(spec.clone()).unwrap();
    second.transform(0, &request).unwrap();
    assert_eq!(spawned(), 1);
    first.close().unwrap();
    assert!(connection_ended
        .recv_timeout(Duration::from_millis(200))
        .is_err());
    second.close().unwrap();
    connection_ended
        .recv_timeout(Duration::from_secs(10))
        .expect("releasing the last lease ends the mapper's connection");
    let third = host.project(spec.clone()).unwrap();
    third.transform(0, &request).unwrap();
    assert_eq!(spawned(), 2);
    host.close().unwrap();
    connection_ended
        .recv_timeout(Duration::from_secs(10))
        .expect("closing the host ends a leased mapper's connection");
    assert!(host.project(spec).is_none());
    assert!(third.transform(0, &request).is_err());
}
