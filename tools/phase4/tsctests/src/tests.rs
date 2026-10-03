//! Unit tests of the ported harness pieces.
//!
//! Expectations come from the pin, never from this port:
//!
//! - `testdata/probe.json`, captured from the pinned functions by the
//!   access-only overlay `probe/harness_probe_test.go`
//!   (`probe/capture.py`): the output sanitizer, the symbol-name sanitizer,
//!   the baselining tracer, the mock watch backend, the clock, the readable
//!   build info on synthetic build infos, the file-system differ and
//!   `testFs` driven step by step, the testing hooks and
//!   `getDiffForIncremental`. Each test replays the fixture's own inputs.
//! - the committed references under `upstream/tsc/testdata/baselines`: every
//!   build info with a readable rendering beside it is rendered again from its
//!   text, and every recorded scenario's transcript up to its first command
//!   is rendered again from the recording.
use crate::execute::fswatch;
use crate::execute::tsc::{CommandLineTesting, SharedWriter, System, Writer};
use crate::execute::watchmanager::{Closer, WatchBackend, WatchDirectoryRequest};
use crate::fsbaselineutil::{sanitize_internal_symbol_name, FileChange};
use crate::goutil::StringBuilder;
use crate::harnessutil::{ComparePathsOptions, TracerForBaselining};
use crate::mock_watch_backend::{new_mock_watch_backend, path_is_under};
use crate::readablebuildinfo::{to_readable_build_info, to_readable_file_emit_kind};
use crate::runner::{exit_status_line, get_diff_for_incremental, TscInput};
use crate::scenario::{apply, read_inventory, Inventory, Operation, OperationKind};
use crate::sys::{
    new_test_sys, sanitize_output, FileMap, TestClock, TestSys, TSC_DEFAULT_LIB_CONTENT,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use tsr_core::collections::SyncMap;
use tsr_diagnostics::Argument;
use tsr_incremental::{BuildInfo, FileEmitKind};
use tsr_jsstring::JsString;
use tsr_vfs::iofs::Time;
use tsr_vfs::vfstest::{self, InputFile};

const PLACEHOLDER: &str = "{{VERSION}}";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn references() -> PathBuf {
    root().join("upstream/tsc/testdata/baselines/reference")
}

fn probe() -> &'static Value {
    static PROBE: OnceLock<Value> = OnceLock::new();
    PROBE.get_or_init(|| {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/probe.json");
        serde_json::from_slice(&std::fs::read(path).expect("the probe fixture exists"))
            .expect("the probe fixture is JSON")
    })
}

fn results(section: &str) -> &'static Value {
    &probe()["results"][section]
}

fn inventory() -> &'static Inventory {
    static INVENTORY: OnceLock<Inventory> = OnceLock::new();
    INVENTORY.get_or_init(|| {
        read_inventory(&root().join("data/phase4/scenarios.json.gz"))
            .expect("the recorded scenarios read")
    })
}

fn expand(text: &str) -> Vec<u8> {
    text.replace(PLACEHOLDER, tsr_core::version()).into_bytes()
}

/// `{"text"}` or `{"hex"}` bytes.
fn bytes_of(value: &Value) -> Vec<u8> {
    if let Some(text) = value.get("text").and_then(Value::as_str) {
        return text.as_bytes().to_vec();
    }
    let hex = value["hex"].as_str().expect("text or hex");
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex"))
        .collect()
}

fn text_of(bytes: &[u8]) -> Value {
    match std::str::from_utf8(bytes) {
        Ok(text) => json!({ "text": text }),
        Err(_) => json!({ "hex": crate::scenario::hex(bytes) }),
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_else(|| "non-string panic payload".to_owned())
}

/// The pin's `phase4Try`: `{"result": text}` or `{"panic": message}`.
fn attempt(body: impl FnOnce() -> Vec<u8>) -> Value {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(text) => json!({ "result": text_of(&text) }),
        Err(payload) => json!({ "panic": panic_message(payload.as_ref()) }),
    }
}

fn digest(bytes: &[u8]) -> String {
    crate::scenario::hex_digest(bytes)
}

#[test]
fn probe_fixture_is_bound_to_the_pin_and_the_overlay() {
    let pin: Value =
        serde_json::from_slice(&std::fs::read(root().join("data/upstream.json")).unwrap()).unwrap();
    assert_eq!(probe()["pin"], pin["pin"]);
    let overlay =
        std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("probe/harness_probe_test.go"))
            .unwrap();
    assert_eq!(
        probe()["overlay_sha256"],
        digest(&overlay),
        "rerun probe/capture.py"
    );
    assert_eq!(results("version_placeholder"), PLACEHOLDER);
}

#[test]
fn output_sanitizer_matches_the_pin() {
    let cases = results("sanitizer").as_array().unwrap();
    assert!(cases.len() > 20);
    for case in cases {
        let input = bytes_of(&case["input"]);
        assert_eq!(
            attempt(|| sanitize_output(&input, false)),
            case["output"],
            "{case}"
        );
        assert_eq!(
            attempt(|| sanitize_output(&input, true)),
            case["comparing"],
            "{case}"
        );
    }
}

#[test]
fn internal_symbol_names_match_the_pin() {
    for case in results("symbol_names").as_array().unwrap() {
        let input = bytes_of(&case["input"]);
        let output = sanitize_internal_symbol_name(&input);
        assert_eq!(text_of(&output), case["output"], "{case}");
    }
}

#[test]
fn baselining_tracer_matches_the_pin() {
    for case in results("tracer").as_array().unwrap() {
        let builder = Arc::new(StringBuilder::new());
        let other = StringBuilder::new();
        let tracer = TracerForBaselining::new(
            ComparePathsOptions {
                use_case_sensitive_file_names: case["case_sensitive"].as_bool().unwrap(),
                current_directory: case["cwd"].as_str().unwrap().as_bytes().to_vec(),
            },
            builder.clone(),
        );
        for step in case["steps"].as_array().unwrap() {
            let msg = expand(step["msg"].as_str().unwrap_or_default());
            let use_cache = step["use_cache"].as_bool().unwrap_or_default();
            if step["reset"] == true {
                tracer.reset();
            } else if step["other"] == true {
                tracer.trace_with_writer(&other, &msg, use_cache);
            } else {
                tracer.trace_with_writer(&*builder, &msg, use_cache);
            }
        }
        let argument = [Argument::Bytes(b"/z/package.json".to_vec())];
        tracer.trace(tsr_diagnostics::File_0_does_not_exist, &argument);
        tracer.trace(tsr_diagnostics::File_0_does_not_exist, &argument);
        assert_eq!(
            text_of(&tracer.string()),
            json!({"text": case["builder"]}),
            "{case}"
        );
        assert_eq!(
            text_of(&other.string()),
            json!({"text": case["other"]}),
            "{case}"
        );
    }
}

#[test]
fn mock_watch_backend_matches_the_pin() {
    for case in results("watch").as_array().unwrap() {
        let existing: Vec<Vec<u8>> = case["existing"]
            .as_array()
            .unwrap()
            .iter()
            .map(|dir| dir.as_str().unwrap().as_bytes().to_vec())
            .collect();
        let backend = new_mock_watch_backend(
            Some(Arc::new(move |dir: &[u8]| {
                existing.iter().any(|e| e == dir)
            })),
            case["case_sensitive"].as_bool().unwrap(),
        );
        let received = Arc::new(Mutex::new(Vec::<String>::new()));
        let mut closers: BTreeMap<String, Arc<dyn Closer>> = BTreeMap::new();
        let mut outputs = Vec::new();
        let take_sorted = |received: &Mutex<Vec<String>>| {
            let mut seen =
                std::mem::take(&mut *received.lock().unwrap_or_else(PoisonError::into_inner));
            seen.sort();
            if seen.is_empty() {
                Value::Null
            } else {
                json!(seen)
            }
        };
        for step in case["steps"].as_array().unwrap() {
            match step["op"].as_str().unwrap() {
                "has_watches" => outputs.push(json!(backend.has_watches())),
                "state" => outputs.push(json!(String::from_utf8(backend.watch_state()).unwrap())),
                "watch" => {
                    let requests: Vec<WatchDirectoryRequest> = step["requests"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|request| {
                            let dir = request["dir"].as_str().unwrap().to_owned();
                            let sink = received.clone();
                            let watch = dir.clone();
                            let callback: fswatch::WatchCallback = Arc::new(
                                move |events: &[fswatch::Event], error: Option<&fswatch::Error>| {
                                    let mut sink =
                                        sink.lock().unwrap_or_else(PoisonError::into_inner);
                                    if let Some(error) = error {
                                        sink.push(format!("{watch}|error|{error}"));
                                    }
                                    for (index, event) in events.iter().enumerate() {
                                        sink.push(format!(
                                            "{watch}|{index:03}|{}|{}",
                                            event.kind,
                                            String::from_utf8_lossy(&event.path)
                                        ));
                                    }
                                },
                            );
                            let ignore = request["ignore"].as_str().map(|needle| {
                                let needle = needle.as_bytes().to_vec();
                                Arc::new(move |path: &[u8]| crate::goutil::contains(path, &needle))
                                    as crate::execute::watchmanager::Ignore
                            });
                            WatchDirectoryRequest {
                                dir: dir.into_bytes(),
                                callback,
                                recursive: request["recursive"].as_bool().unwrap(),
                                ignore,
                            }
                        })
                        .collect();
                    let dirs: Vec<String> = requests
                        .iter()
                        .map(|request| String::from_utf8(request.dir.clone()).unwrap())
                        .collect();
                    outputs.push(match backend.watch_directories(requests) {
                        Ok(created) => {
                            for (dir, closer) in dirs.into_iter().zip(created) {
                                closers.insert(dir, closer);
                            }
                            json!("ok")
                        }
                        Err(error) => json!(format!("error: {error}")),
                    });
                }
                "close" => {
                    let closer = &closers[step["dir"].as_str().unwrap()];
                    outputs.push(json!(match closer.close() {
                        Ok(()) => "<nil>".to_owned(),
                        Err(error) => error.to_string(),
                    }));
                }
                "send_changed_paths" => {
                    let changes: Vec<FileChange> = step["changes"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|change| FileChange {
                            path: change["path"].as_str().unwrap().as_bytes().to_vec(),
                            deleted: change["deleted"].as_bool().unwrap_or_default(),
                        })
                        .collect();
                    take_sorted(&received);
                    backend.send_changed_paths(&changes);
                    outputs.push(take_sorted(&received));
                }
                "overflow" => {
                    take_sorted(&received);
                    backend.send_overflow();
                    outputs.push(take_sorted(&received));
                }
                other => panic!("unknown watch step {other}"),
            }
        }
        assert_eq!(Value::Array(outputs), case["outputs"], "{case}");
    }
}

#[test]
fn path_is_under_matches_the_pin() {
    for case in results("path_is_under").as_array().unwrap() {
        let under = path_is_under(
            case["event"].as_str().unwrap().as_bytes(),
            case["dir"].as_str().unwrap().as_bytes(),
            case["recursive"].as_bool().unwrap(),
            case["case_sensitive"].as_bool().unwrap(),
        );
        assert_eq!(json!(under), case["under"], "{case}");
    }
}

#[test]
fn test_clock_matches_the_pin() {
    let start = Time::from_unix(1_700_000_000, 500);
    let clock = TestClock::new(start);
    let seconds = |time: Time| time.unix().0 - start.unix().0;
    let mut readings = Vec::new();
    for _ in 0..3 {
        readings.push(seconds(clock.now()));
    }
    let since = clock.since_start();
    readings.push(seconds(clock.now()));
    let expected = results("clock");
    assert_eq!(json!(readings), expected["readings"]);
    assert_eq!(json!(since.as_nanos() as u64), expected["since_start_ns"]);
    assert_eq!(clock.readings(), 5);
    assert_eq!(
        clock.now().unix().1,
        500,
        "a reading keeps the start's nanoseconds"
    );
}

#[test]
fn baseline_sub_folders_match_the_pin() {
    for case in results("sub_folders").as_array().unwrap() {
        let input = TscInput {
            command_line_args: case["args"]
                .as_array()
                .map(|args| {
                    args.iter()
                        .map(|arg| JsString::from_bytes(arg.as_str().unwrap().as_bytes()))
                        .collect()
                })
                .unwrap_or_default(),
            ..TscInput::default()
        };
        assert_eq!(
            json!(input.get_baseline_sub_folder()),
            case["folder"],
            "{case}"
        );
    }
}

#[test]
fn readable_file_emit_kinds_match_the_pin() {
    let expected = results("emit_kinds").as_array().unwrap();
    assert_eq!(expected.len(), 64);
    for (kind, name) in expected.iter().enumerate() {
        assert_eq!(
            json!(to_readable_file_emit_kind(FileEmitKind(kind as u32))),
            *name
        );
    }
}

#[test]
fn terminal_width_matches_the_pin() {
    for case in results("terminal_width").as_array().unwrap() {
        let mut env = BTreeMap::new();
        if let Some(value) = case["value"].as_str() {
            env.insert("TS_TEST_TERMINAL_WIDTH".to_owned(), value.to_owned());
        }
        let sys = new_test_sys(
            &TscInput {
                env,
                ..TscInput::default()
            },
            false,
        );
        match catch_unwind(AssertUnwindSafe(|| sys.get_width_of_terminal())) {
            Ok(width) => assert_eq!(json!(width), case["width"], "{case}"),
            Err(payload) => assert_eq!(
                json!(panic_message(payload.as_ref())),
                case["panic"],
                "{case}"
            ),
        }
    }
}

fn unmarshal_build_info(text: &[u8]) -> Result<BuildInfo, tsr_json::Error> {
    let mut build_info = BuildInfo::default();
    tsr_json::unmarshal(text, &mut build_info, tsr_json::Options::default())?;
    Ok(build_info)
}

#[test]
fn file_text_edits_allow_an_empty_search_string() {
    let path = b"/a.ts";
    let sys = new_test_sys(
        &TscInput {
            files: BTreeMap::from([(path.to_vec(), InputFile::Text("aé".as_bytes().to_vec()))]),
            ..TscInput::default()
        },
        false,
    );
    sys.replace_file_text(path, b"", b"!");
    assert_eq!(sys.read_file_no_error(path), "!aé".as_bytes());
    sys.replace_file_text_all(path, b"", b"-");
    assert_eq!(sys.read_file_no_error(path), "-!-a-é-".as_bytes());
}

#[test]
fn readable_build_info_matches_the_pin_on_synthetic_build_infos() {
    for case in results("readable").as_array().unwrap() {
        let text = case["text"].as_str().unwrap().as_bytes();
        assert!(case.get("error").is_none(), "the pin rejects {case}");
        let build_info = unmarshal_build_info(text).expect("the build info decodes");
        assert_eq!(
            attempt(|| to_readable_build_info(&build_info, text)),
            case["readable"],
            "{case}"
        );
    }
}

/// Native `toReadableBuildInfo` copies or maps non-nil empty slices to
/// non-nil empty slices, except referencedMap, which becomes an empty object.
/// These cases were executed against the pin with an access-only Go overlay.
#[test]
fn readable_build_info_preserves_null_and_empty_collection_shapes() {
    for field in [
        "root",
        "packageJsons",
        "missingPackageJsons",
        "contentMapperIdentities",
        "fileNames",
        "fileInfos",
        "fileIdsList",
        "referencedMap",
        "semanticDiagnosticsPerFile",
        "emitDiagnosticsPerFile",
        "changeFileSet",
        "affectedFilesPendingEmit",
        "emitSignatures",
        "resolvedRoot",
        "options",
    ] {
        let empty = if field == "options" { "{}" } else { "[]" };
        for value in ["null", empty] {
            let text = format!("{{\"{field}\":{value}}}");
            let build_info = unmarshal_build_info(text.as_bytes()).unwrap();
            let readable_value = if field == "referencedMap" {
                "{}"
            } else {
                empty
            };
            let member = if value == "null" || field == "contentMapperIdentities" {
                String::new()
            } else {
                format!("  \"{field}\": {readable_value},\n")
            };
            let expected = format!("{{\n{member}  \"size\": {}\n}}", text.len());
            assert_eq!(
                to_readable_build_info(&build_info, text.as_bytes()),
                expected.as_bytes(),
                "{text}"
            );
        }
    }
}

#[test]
fn readable_build_info_preserves_nested_diagnostic_collection_shapes() {
    for field in ["messageArgs", "messageChain", "relatedInformation"] {
        for value in ["null", "[]"] {
            let text = format!(
                "{{\"fileNames\":[\"./a.ts\"],\"semanticDiagnosticsPerFile\":[[1,[{{\"{field}\":{value}}}]]]}}"
            );
            let build_info = unmarshal_build_info(text.as_bytes()).unwrap();
            let diagnostic = if value == "null" {
                json!({})
            } else {
                json!({field: []})
            };
            let rendered = to_readable_build_info(&build_info, text.as_bytes());
            assert_eq!(
                serde_json::from_slice::<Value>(&rendered).unwrap(),
                json!({
                    "fileNames": ["./a.ts"],
                    "semanticDiagnosticsPerFile": [["./a.ts", [diagnostic]]],
                    "size": text.len()
                }),
                "{text}"
            );
        }
    }
}

/// The build-info sections of a committed reference: `(path, text)` for every
/// file-system entry whose text is shown (`*new*` or `*modified*`). A build
/// info is one line; a readable rendering ends at its closing `}`.
fn shown_sections(text: &str) -> Vec<(String, String)> {
    let mut sections = Vec::new();
    let lines: Vec<&str> = text.split('\n').collect();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        index += 1;
        let Some(rest) = line.strip_prefix("//// [") else {
            continue;
        };
        let Some((path, label)) = rest.split_once("] ") else {
            continue;
        };
        if label != "*new* " && label != "*modified* " {
            continue;
        }
        if path.ends_with(".tsbuildinfo") {
            let start = index;
            if lines[index] == "{" {
                while lines[index].trim() != "}" {
                    index += 1;
                }
            }
            sections.push((path.to_owned(), lines[start..=index].join("\n")));
        } else if path.ends_with(".tsbuildinfo.readable.baseline.txt") {
            let start = index;
            while lines[index] != "}" {
                index += 1;
            }
            sections.push((path.to_owned(), lines[start..=index].join("\n")));
        }
    }
    sections
}

fn reference_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for family in crate::scenario::FAMILIES {
        let mut stack = vec![references().join(family)];
        while let Some(directory) = stack.pop() {
            for entry in std::fs::read_dir(&directory).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    files.push(path);
                }
            }
        }
    }
    files.sort();
    files
}

/// Every readable rendering in the committed references, rendered again
/// from the build-info text it was written for (the latest shown text of the
/// same build info in the same reference).
#[test]
fn readable_build_info_renders_every_committed_rendering() {
    use sha2::{Digest, Sha256};
    let (mut build_infos, mut readable, mut matched) = (0, 0, 0);
    let mut mismatches = Vec::new();
    for file in reference_files() {
        let text = std::fs::read_to_string(&file).unwrap();
        let mut latest: BTreeMap<String, String> = BTreeMap::new();
        for (path, section) in shown_sections(&text) {
            if let Some(build_info_path) = path.strip_suffix(".readable.baseline.txt") {
                readable += 1;
                let Some(build_info_text) = latest.get(build_info_path) else {
                    mismatches.push(format!("{}: {path} has no build info", file.display()));
                    continue;
                };
                let rendered = unmarshal_build_info(build_info_text.as_bytes()).map(|build_info| {
                    to_readable_build_info(&build_info, build_info_text.as_bytes())
                });
                match rendered {
                    Ok(rendered)
                        if sanitize_internal_symbol_name(&rendered) == section.as_bytes() =>
                    {
                        matched += 1;
                    }
                    Ok(_) => {
                        mismatches.push(format!("{}: {path} renders differently", file.display()));
                    }
                    Err(error) => mismatches.push(format!("{}: {path}: {error}", file.display())),
                }
            } else {
                build_infos += 1;
                let input_hash = format!("{:x}", Sha256::digest(section.as_bytes()));
                let expected = &results("buildinfo_codec")[&input_hash];
                assert!(
                    !expected.is_null(),
                    "{}: {path} is not in the native codec observation",
                    file.display()
                );
                let decoded = unmarshal_build_info(section.as_bytes());
                assert_eq!(
                    decoded.is_err(),
                    expected["error"].as_bool().unwrap(),
                    "{}: {path}",
                    file.display()
                );
                if let Ok(decoded) = decoded {
                    let encoded =
                        tsr_json::marshal(&decoded, tsr_json::Options::default()).unwrap();
                    assert_eq!(
                        format!("{:x}", Sha256::digest(&encoded)),
                        expected["sha256"].as_str().unwrap(),
                        "{}: {path}",
                        file.display()
                    );
                }
                latest.insert(path, section);
            }
        }
    }
    eprintln!("build-info texts {build_infos}, readable renderings {readable}, matched {matched}");
    assert!(
        mismatches.is_empty(),
        "{} mismatches: {mismatches:#?}",
        mismatches.len()
    );
    assert_eq!(matched, readable);
    assert_eq!(build_infos, 1_271, "the pinned codec denominator changed");
    assert_eq!(readable, 1_257, "the pinned rendering denominator changed");
}

/// Builds the fake system of a probe case.
fn probe_sys(case: &Value) -> Arc<TestSys> {
    let mut files = FileMap::new();
    for (path, text) in case["files"].as_object().unwrap() {
        files.insert(
            path.as_bytes().to_vec(),
            InputFile::Text(expand(text.as_str().unwrap())),
        );
    }
    if let Some(symlinks) = case["symlinks"].as_object() {
        for (path, target) in symlinks {
            files.insert(
                path.as_bytes().to_vec(),
                InputFile::File(vfstest::symlink(target.as_str().unwrap().as_bytes())),
            );
        }
    }
    new_test_sys(
        &TscInput {
            files,
            cwd: case["cwd"].as_str().unwrap().as_bytes().to_vec(),
            ignore_case: case["ignore_case"].as_bool().unwrap(),
            windows_style_root: case["windows_style_root"]
                .as_str()
                .unwrap()
                .as_bytes()
                .to_vec(),
            ..TscInput::default()
        },
        false,
    )
}

fn fs_step(sys: &Arc<TestSys>, step: &Value) -> Vec<u8> {
    let path = step["path"].as_str().unwrap_or_default().as_bytes();
    let text = expand(step["text"].as_str().unwrap_or_default());
    let paths = |key: &str| -> Vec<String> {
        step[key]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| item.as_str().unwrap().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    };
    match step["op"].as_str().unwrap() {
        "baseline" => {
            let mut builder = Vec::new();
            sys.baseline_fs_with_diff(&mut builder);
            return builder;
        }
        "changed_paths" => {
            let changes = sys.fs_differ.changed_paths();
            let mut lines: Vec<String> = changes
                .iter()
                .filter(|change| !change.deleted)
                .map(|change| format!("changed {}", String::from_utf8_lossy(&change.path)))
                .collect();
            let mut deleted: Vec<String> = changes
                .iter()
                .filter(|change| change.deleted)
                .map(|change| format!("deleted {}", String::from_utf8_lossy(&change.path)))
                .collect();
            deleted.sort();
            lines.extend(deleted);
            return lines.join("\n").into_bytes();
        }
        "write" => sys.write_file_no_error(path, &text),
        "fs_write" => {
            if let Err(error) = sys.fs().write_file(path, &text) {
                return format!("error: {error}").into_bytes();
            }
        }
        "fs_read" => {
            return match sys.fs().read_file(path).unwrap() {
                None => b"<missing>".to_vec(),
                Some(content) => crate::goutil::replace_all(
                    &content.raw,
                    tsr_core::version().as_bytes(),
                    PLACEHOLDER.as_bytes(),
                ),
            }
        }
        "remove" => sys.remove_no_error(path),
        "fs_remove" => {
            if let Err(error) = sys.fs().remove(path) {
                return format!("error: {error}").into_bytes();
            }
        }
        "touch" => {
            let now = System::now(&**sys);
            if let Err(error) = sys.fs().change_times(path, Time::ZERO, now) {
                return format!("error: {error}").into_bytes();
            }
        }
        "symlink" => sys
            .map_fs()
            .add_symlink(path, step["target"].as_str().unwrap().as_bytes()),
        "readings" => return sys.clock.readings().to_string().into_bytes(),
        "written" => {
            return sys.fs.written_files.to_slice().join(&b'\n');
        }
        "default_libs" => {
            return match sys.fs.default_libs.snapshot() {
                None => b"<nil>".to_vec(),
                Some(libs) => libs.len().to_string().into_bytes(),
            }
        }
        "emitted" => {
            let cache: SyncMap<JsString, Time> = SyncMap::default();
            let cached = paths("cache");
            for path in &cached {
                cache.store(JsString::from_bytes(path.as_bytes()), Time::ZERO);
            }
            let before = sys.clock.readings();
            let result = tsr_compiler::EmitResult {
                emitted_files: paths("files")
                    .iter()
                    .map(|file| JsString::from_bytes(file.as_bytes()))
                    .collect(),
                ..tsr_compiler::EmitResult::default()
            };
            sys.on_emitted_files(Some(&result), Some(&cache));
            let entries: Vec<String> = cached
                .iter()
                .map(|path| {
                    let value = cache.load(path.as_bytes()).unwrap();
                    if value.is_zero() {
                        format!("{path}=zero")
                    } else {
                        let reading = value.unix().0 - sys.clock.start().unix().0;
                        format!("{path}=+{}", reading - before as i64)
                    }
                })
                .collect();
            return format!(
                "readings +{}\n{}",
                sys.clock.readings() - before,
                entries.join("\n")
            )
            .into_bytes();
        }
        "emitted_nil" => sys.on_emitted_files(None, None),
        "trace" => {
            let other = Arc::new(StringBuilder::new());
            let other_writer: SharedWriter = other.clone();
            let trace = sys.get_trace(sys.writer(), tsr_locale::Locale::default());
            let found = [Argument::Bytes(b"/a/package.json".to_vec())];
            trace(tsr_diagnostics::Found_package_json_at_0, &found);
            trace(tsr_diagnostics::Found_package_json_at_0, &found);
            let trace_other = sys.get_trace(other_writer.clone(), tsr_locale::Locale::default());
            let missing = [Argument::Bytes(b"/b/package.json".to_vec())];
            trace_other(tsr_diagnostics::File_0_does_not_exist, &missing);
            trace_other(tsr_diagnostics::File_0_does_not_exist, &missing);
            sys.on_list_files_start(&other_writer);
            sys.on_list_files_end(&other_writer);
            sys.on_statistics_start(&other_writer);
            sys.on_statistics_end(&other_writer);
            sys.on_build_status_report_start(&other_writer);
            sys.on_build_status_report_end(&other_writer);
            sys.on_watch_status_report_start();
            sys.on_watch_status_report_end();
            return [
                sys.current_write().string(),
                b"|other|".to_vec(),
                other.string(),
            ]
            .concat();
        }
        "output" => return sys.get_output(false),
        "clear_output" => sys.clear_output(),
        "write_output" => {
            sys.writer().write(&text).unwrap();
        }
        other => panic!("unknown fake-system step {other}"),
    }
    Vec::new()
}

/// The pin panics with the decoder's own error after this prefix; the Rust
/// decoder words its errors differently.
const UNMARSHAL_PANIC: &str = "testFs.WriteFile: failed to unmarshal build info: - use underlying FS's write method if this is intended use for testcase";

#[test]
fn fake_system_steps_match_the_pin() {
    for entry in results("fs").as_array().unwrap() {
        let case = &entry["case"];
        let sys = probe_sys(case);
        let steps = case["steps"].as_array().unwrap();
        let expected = entry["outputs"].as_array().unwrap();
        assert_eq!(steps.len(), expected.len());
        for (index, (step, expected)) in steps.iter().zip(expected).enumerate() {
            let mut actual = attempt(|| fs_step(&sys, step));
            if let (Some(want), Some(got)) = (expected["panic"].as_str(), actual["panic"].as_str())
            {
                if want.starts_with(UNMARSHAL_PANIC) && got.starts_with(UNMARSHAL_PANIC) {
                    actual = expected.clone();
                }
            }
            assert_eq!(actual, *expected, "step {index} {step}");
        }
    }
}

#[test]
fn incremental_differences_match_the_pin() {
    for case in results("incremental_diff").as_array().unwrap() {
        let build = |side: &Value, shadow: bool| {
            let sys = new_test_sys(
                &TscInput {
                    files: FileMap::from([(
                        b"/p/tsconfig.json".to_vec(),
                        InputFile::Text(b"{}".to_vec()),
                    )]),
                    cwd: b"/p".to_vec(),
                    ..TscInput::default()
                },
                shadow,
            );
            for write in side["writes"].as_array().into_iter().flatten() {
                let path = write[0].as_str().unwrap().as_bytes();
                sys.fs()
                    .write_file(path, &expand(write[1].as_str().unwrap()))
                    .unwrap();
            }
            for write in side["raw"].as_array().into_iter().flatten() {
                sys.write_file_no_error(
                    write[0].as_str().unwrap().as_bytes(),
                    &expand(write[1].as_str().unwrap()),
                );
            }
            sys.writer()
                .write(side["output"].as_str().unwrap().as_bytes())
                .unwrap();
            sys
        };
        let actual = attempt(|| {
            get_diff_for_incremental(
                &build(&case["incremental"], false),
                &build(&case["non_incremental"], true),
            )
        });
        assert_eq!(actual, case["diff"], "{case}");
    }
}

#[test]
fn library_text_is_the_pins() {
    // The hash the committed references record for every default library.
    assert_eq!(
        tsr_incremental::compute_hash(TSC_DEFAULT_LIB_CONTENT.as_bytes(), false).as_bytes(),
        b"8859c12c614ce56ba9a18e58384a198f"
    );
    assert_eq!(inventory().library_text, TSC_DEFAULT_LIB_CONTENT.as_bytes());
}

/// The text of a committed reference before its first command, and the first
/// command's line.
fn header_of(reference: &[u8]) -> &[u8] {
    let start =
        crate::goutil::index(reference, b"\ntsgo ").expect("a reference runs a command") + 1;
    &reference[..start]
}

#[test]
fn every_scenario_renders_the_references_header() {
    let inventory = inventory();
    assert_eq!(inventory.scenarios.len(), 516);
    let mut ids: Vec<String> = inventory.scenarios.iter().map(|s| s.id.clone()).collect();
    ids.extend(inventory.orphan_references.iter().cloned());
    ids.sort();
    let mut files: Vec<String> = reference_files()
        .iter()
        .map(|file| {
            file.strip_prefix(references())
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    files.sort();
    assert_eq!(
        ids, files,
        "the scenarios and orphans are the four families"
    );
    let mut matched = 0;
    for scenario in &inventory.scenarios {
        let input = scenario.to_tsc_input();
        let sys = new_test_sys(&input, false);
        let mut transcript = Vec::new();
        crate::runner::write_header(&sys, &mut transcript);
        let reference = std::fs::read(references().join(&scenario.id)).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&transcript),
            String::from_utf8_lossy(header_of(&reference)),
            "{}",
            scenario.id
        );
        matched += 1;
    }
    assert_eq!(matched, 516);
}

#[test]
fn exit_status_lines_are_the_runners() {
    use crate::execute::tsc::ExitStatus;
    for (status, line) in [
        (ExitStatus::Success, "ExitStatus:: Success"),
        (
            ExitStatus::DiagnosticsPresent_OutputsSkipped,
            "ExitStatus:: DiagnosticsPresent_OutputsSkipped",
        ),
        (
            ExitStatus::DiagnosticsPresent_OutputsGenerated,
            "ExitStatus:: DiagnosticsPresent_OutputsGenerated",
        ),
        (
            ExitStatus::InvalidProject_OutputsSkipped,
            "ExitStatus:: InvalidProject_OutputsSkipped",
        ),
        (
            ExitStatus::ProjectReferenceCycle_OutputsSkipped,
            "ExitStatus:: ProjectReferenceCycle_OutputsSkipped",
        ),
        (ExitStatus::NotImplemented, "ExitStatus:: NotImplemented"),
    ] {
        assert_eq!(exit_status_line(status), line.as_bytes());
    }
    let unknown = catch_unwind(|| exit_status_line(ExitStatus(6))).unwrap_err();
    assert_eq!(panic_message(unknown.as_ref()), "UnknownExitStatus 6");
}

#[test]
fn replay_requires_the_recorded_clock_readings() {
    let sys = new_test_sys(&TscInput::default(), false);
    let write = |clock_readings| Operation {
        kind: OperationKind::Write(b"x".to_vec()),
        path: b"/home/src/tslibs/TS/Lib/new/dir/a.ts".to_vec(),
        clock_readings,
    };
    // The parent and grandparent are created, then the file is written.
    apply(&sys, &write(3));
    let wrong = catch_unwind(AssertUnwindSafe(|| apply(&sys, &write(2)))).unwrap_err();
    assert!(panic_message(wrong.as_ref()).contains("read the clock 1 times, recorded 2"));
    let touch = Operation {
        kind: OperationKind::Chtimes,
        path: b"/home/src/tslibs/TS/Lib/new/dir/a.ts".to_vec(),
        clock_readings: 1,
    };
    apply(&sys, &touch);
}

fn reject(document: &Value) -> String {
    let mut bytes = crate::scenario::canonical_json(document);
    bytes.push(b'\n');
    crate::scenario::parse_inventory(&bytes).expect_err("the reader rejects the document")
}

#[test]
fn scenario_reader_rejects_malformed_inventories() {
    let path = root().join("data/phase4/scenarios.json.gz");
    let output = std::process::Command::new("gzip")
        .arg("-dc")
        .arg(&path)
        .output()
        .unwrap();
    let original: Value = serde_json::from_slice(&output.stdout).unwrap();
    let first = original["scenarios"][0]["id"].as_str().unwrap().to_owned();
    let mutate = |change: &dyn Fn(&mut Value)| {
        let mut document = original.clone();
        change(&mut document);
        reject(&document)
    };
    assert!(mutate(&|d| d["scenarios"][0]["extra"] = json!(1)).contains("unknown key extra"));
    assert!(mutate(&|d| d["scenarios"][0]["cwd"] = json!("/elsewhere")).contains("digest"));
    assert!(mutate(&|d| {
        d["provenance"]["scenario_digests"]
            .as_object_mut()
            .unwrap()
            .remove(&first);
    })
    .contains("no recorded digest"));
    assert!(mutate(&|d| d["version"] = json!(2)).contains("version-1"));
    assert!(mutate(&|d| d["library_text"] = json!("x")).contains("library text"));
    assert!(mutate(&|d| d["scenarios"].as_array_mut().unwrap().swap(0, 1)).contains("sorted"));
    // A text stored as hex although it is UTF-8, and a write with no reading.
    let edited = original["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .position(|s| {
            s["edits"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| !e["operations"].as_array().unwrap().is_empty())
        })
        .unwrap();
    let rebind = |d: &mut Value| {
        let id = d["scenarios"][edited]["id"].as_str().unwrap().to_owned();
        let digest =
            crate::scenario::hex_digest(&crate::scenario::canonical_json(&d["scenarios"][edited]));
        d["provenance"]["scenario_digests"][&id] = json!(digest);
    };
    let error = mutate(&|d| {
        let edit = d["scenarios"][edited]["edits"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|e| !e["operations"].as_array().unwrap().is_empty())
            .unwrap();
        edit["operations"][0] =
            json!({"op": "write", "path": "/a", "text_hex": "61", "clock_readings": 1});
        rebind(d);
    });
    assert!(error.contains("UTF-8 bytes as text_hex"), "{error}");
    let error = mutate(&|d| {
        let edit = d["scenarios"][edited]["edits"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|e| !e["operations"].as_array().unwrap().is_empty())
            .unwrap();
        edit["operations"][0] =
            json!({"op": "write", "path": "/a", "text": "a", "clock_readings": 0});
        rebind(d);
    });
    assert!(error.contains("0 clock readings for a write"), "{error}");
}

#[test]
fn a_writer_is_compared_by_identity() {
    let one: SharedWriter = Arc::new(StringBuilder::new());
    let same = one.clone();
    let other: SharedWriter = Arc::new(StringBuilder::new());
    assert!(crate::execute::tsc::same_writer(&one, &same));
    assert!(!crate::execute::tsc::same_writer(&one, &other));
    let _ = Writer::write(&*one, b"x");
}

#[test]
fn rows_classify_refusals_panics_and_harness_defects() {
    crate::row::install_panic_hook();
    let refusal = tsr_compiler::Error::Unsupported("an operation");
    assert_eq!(
        crate::row::command_error(&refusal),
        json!({"state": "unsupported", "operation": "an operation"})
    );
    let reason = format!("a probe panic {:?}", std::thread::current().id());
    let panicked = catch_unwind(|| panic!("{reason}")).unwrap_err();
    let state = crate::row::unwound(panicked.as_ref());
    assert_eq!(state["state"], "failed");
    assert_eq!(state["class"], "panic");
    assert_eq!(state["reason"], json!(reason));
    assert!(
        state["location"]
            .as_str()
            .is_some_and(|at| at.contains("tests.rs")),
        "{state}"
    );
    // A scenario that does not replay as recorded is the harness's defect.
    let mut scenario = inventory().scenarios[0].clone();
    scenario.initial_clock_readings += 1;
    let (row, transcript) = crate::row::run_scenario(&scenario);
    assert_eq!(row["state"], "failed");
    assert_eq!(row["class"], "harness");
    assert_eq!(row["progress"]["stage"], "setup");
    assert!(
        row["reason"].as_str().unwrap().contains("read the clock"),
        "{row}"
    );
    assert!(transcript.is_empty());
}

/// Complete pinned transcripts witness checker alias marking, literal freshness,
/// dependency-bearing casing aliases, and cached incremental rebuilds together.
#[test]
fn command_line_regressions_match_complete_native_scenarios() {
    let prefixes = [
        "tsc/incremental/const-enums",
        "tsc/incremental/change-to-type-that-gets-used-as-global-through-export",
        "tsc/incremental/Compile-incremental-with-case-insensitive-file-names.js",
        "tsc/forceConsistentCasingInFileNames/when-file-is-included-from-multiple-places-with-different-casing.js",
        "tsbuild/outputPaths/when-rootDir-is-specified-but-not-all-files-belong-to-rootDir",
    ];
    let mut count = 0;
    for scenario in &inventory().scenarios {
        if !prefixes
            .iter()
            .any(|prefix| scenario.id.starts_with(prefix))
        {
            continue;
        }
        let (row, transcript) = crate::row::run_scenario(scenario);
        assert_eq!(row["state"], "completed", "{row}");
        let expected = std::fs::read(references().join(&scenario.id)).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&transcript),
            String::from_utf8_lossy(&expected),
            "{}",
            scenario.id
        );
        count += 1;
    }
    assert_eq!(count, 10);
}

#[test]
fn clean_build_refusal_is_returned_after_both_jobs_finish() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let completed = AtomicBool::new(false);
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        crate::runner::parallel_builds(
            || {
                completed.store(true, Ordering::SeqCst);
                Ok(())
            },
            || Err::<(), _>(tsr_compiler::Error::Unsupported("clean build operation")),
        )
    }));
    assert!(completed.load(Ordering::SeqCst));
    assert!(matches!(
        outcome,
        Ok(Err(tsr_compiler::Error::Unsupported(
            "clean build operation"
        )))
    ));
}

/// X3's separate repetition contract: the sample projects still report in
/// build order when four builders race. Full native parity is measured by the
/// scenario capture; this witness compares twenty executions of the same input.
#[test]
fn build_sample_transcripts_are_deterministic_with_four_builders() {
    let samples: Vec<_> = inventory()
        .scenarios
        .iter()
        .filter(|s| s.id.starts_with("tsbuild/sample/"))
        .cloned()
        .map(|mut scenario| {
            let set_builders = |args: &mut Vec<JsString>| {
                args.extend([
                    JsString::from_bytes(b"--builders".as_slice()),
                    JsString::from_bytes(b"4".as_slice()),
                ]);
            };
            set_builders(scenario.command_line_args.as_mut().unwrap());
            for edit in &mut scenario.edits {
                if let Some(args) = &mut edit.command_line_args {
                    set_builders(args);
                }
            }
            scenario
        })
        .collect();
    assert_eq!(samples.len(), 30);
    let mut first = Vec::new();
    for iteration in 0..20 {
        for (index, scenario) in samples.iter().enumerate() {
            let (row, transcript) = crate::row::run_scenario(scenario);
            assert_eq!(row["state"], "completed", "{}: {row}", scenario.id);
            if iteration == 0 {
                first.push(transcript);
            } else {
                assert_eq!(
                    transcript, first[index],
                    "{}: iteration {iteration}",
                    scenario.id
                );
            }
        }
    }
}

#[test]
fn parallel_build_refusals_do_not_hide_genuine_panics() {
    let outcome = catch_unwind(|| {
        crate::runner::parallel_builds(
            || Err(tsr_compiler::Error::Unsupported("incremental operation")),
            || -> Result<(), tsr_compiler::Error> { panic!("actual clean-build failure") },
        )
    });
    let panic = outcome.expect_err("genuine panic is propagated");
    let row = crate::row::unwound(panic.as_ref());
    assert_eq!(row["class"], "panic");
    assert_eq!(row["reason"], "actual clean-build failure");
}
