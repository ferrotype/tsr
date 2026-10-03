use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tsr_vfs::{Entries, FileContent, FileInfo, SnapshotId};

fn fs() -> Arc<dyn FileSystem> {
    Arc::new(tsr_vfs::vfstest::from_map(&BTreeMap::new(), true).into_vfs())
}
fn text(fs: &dyn FileSystem, path: &[u8]) -> String {
    String::from_utf8(fs.read_file(path).unwrap().unwrap().raw.to_vec()).unwrap()
}
fn args(path: &str) -> TraceArgs {
    [("path".into(), TraceValue::Str(path.into()))]
        .into_iter()
        .collect()
}
fn stop(tracing: &Tracing) {
    tracing
        .stop_tracing(|_, _| panic!("no checker was registered"))
        .unwrap();
}

#[test]
fn deterministic_wire_header_and_file_id_match_pinned_baseline() {
    let fs = fs();
    let tr = start_tracing(fs.clone(), b"/trace", b"", true).unwrap();
    let span = tr.span(
        TracePhase::Parse,
        "createSourceFile",
        args("/home/src/workspaces/project/a.ts"),
        true,
    );
    drop(span);
    stop(&tr);
    // File thread ID 354130385 is independently captured in the pin's
    // generateTrace-generates-types-file baseline, not derived by this test.
    assert_eq!(
        text(fs.as_ref(), b"/trace/trace.json"),
        concat!(
            "[\n",
            "{\"pid\":1,\"tid\":1,\"ph\":\"M\",\"cat\":\"__metadata\",\"ts\":1,\"name\":\"process_name\",\"args\":{\"name\":\"tsgo\"}},\n",
            "{\"pid\":1,\"tid\":1,\"ph\":\"M\",\"cat\":\"__metadata\",\"ts\":1,\"name\":\"thread_name\",\"args\":{\"name\":\"Main\"}},\n",
            "{\"pid\":1,\"tid\":1,\"ph\":\"M\",\"cat\":\"disabled-by-default-devtools.timeline\",\"ts\":1,\"name\":\"TracingStartedInBrowser\"},\n",
            "{\"pid\":1,\"tid\":354130385,\"ph\":\"M\",\"cat\":\"__metadata\",\"ts\":1,\"name\":\"thread_name\",\"args\":{\"name\":\"file:/home/src/workspaces/project/a.ts\"}},\n",
            "{\"pid\":1,\"tid\":354130385,\"ph\":\"B\",\"cat\":\"parse\",\"ts\":2,\"name\":\"createSourceFile\",\"args\":{\"path\":\"/home/src/workspaces/project/a.ts\"}},\n",
            "{\"pid\":1,\"tid\":354130385,\"ph\":\"E\",\"cat\":\"parse\",\"ts\":3,\"name\":\"createSourceFile\",\"args\":{\"path\":\"/home/src/workspaces/project/a.ts\"}}\n]\n",
        )
    );
    assert_eq!(text(fs.as_ref(), b"/trace/legend.json"), "[]");
}

#[derive(Default)]
struct ObservedEvent {
    tid: i64,
    ph: String,
    cat: String,
    name: String,
    args: std::collections::HashMap<String, tsr_json::RawValue>,
}
impl tsr_json::Decode for ObservedEvent {
    fn decode(&mut self, input: &mut tsr_json::Decoder<'_>) -> Result<(), tsr_json::Error> {
        input.object(|field, input| match field {
            b"tid" => input.value(&mut self.tid),
            b"ph" => input.value(&mut self.ph),
            b"cat" => input.value(&mut self.cat),
            b"name" => input.value(&mut self.name),
            b"args" => input.object(|name, input| {
                let mut value = tsr_json::RawValue::default();
                input.value(&mut value)?;
                self.args
                    .insert(String::from_utf8(name.to_vec()).unwrap(), value);
                Ok(())
            }),
            _ => input.skip_value(),
        })
    }
}
impl ObservedEvent {
    fn argument<T: tsr_json::Decode + Default>(&self, key: &str) -> Option<T> {
        let raw = self.args.get(key)?;
        let mut value = T::default();
        tsr_json::unmarshal(&raw.0, &mut value, tsr_json::Options::default()).unwrap();
        Some(value)
    }
}
fn observe(fs: &dyn FileSystem) -> Vec<ObservedEvent> {
    let mut events = Vec::new();
    tsr_json::unmarshal(
        text(fs, b"/trace/trace.json").as_bytes(),
        &mut events,
        tsr_json::Options::default(),
    )
    .unwrap();
    events
}

#[test]
// source: tsc/internal/tracing/tracing_test.go:TestConcurrentDurationEventsUseSeparateThreadIDs
fn concurrent_duration_events_have_matching_ends_names_and_per_thread_nesting() {
    let fs = fs();
    let tr = start_tracing(fs.clone(), b"/trace", b"", true).unwrap();
    let a = tr.push(TracePhase::Parse, "createSourceFile", &args("/a.ts"), true);
    let b = tr.push(TracePhase::Parse, "createSourceFile", &args("/b.ts"), true);
    tr.pop(a, &args("/a.ts"));
    tr.pop(b, &args("/b.ts"));
    let mut checker = args("/a.ts");
    checker.insert("checkerId".into(), TraceValue::Int(0));
    let variance: TraceArgs = [
        ("checkerId".into(), TraceValue::Int(0)),
        ("id".into(), TraceValue::Int(1)),
    ]
    .into_iter()
    .collect();
    let check = tr.push(TracePhase::Check, "checkSourceFile", &checker, true);
    let nested = tr.push(
        TracePhase::CheckTypes,
        "getVariancesWorker",
        &variance,
        true,
    );
    tr.pop(nested, &variance);
    tr.pop(check, &checker);
    stop(&tr);
    let events = observe(fs.as_ref());
    let file_id = |phase: &str, path: &str| {
        events
            .iter()
            .find(|event| {
                event.ph == phase
                    && event.name == "createSourceFile"
                    && event.argument::<String>("path").as_deref() == Some(path)
            })
            .unwrap()
            .tid
    };
    assert_eq!(file_id("B", "/a.ts"), file_id("E", "/a.ts"));
    assert_eq!(file_id("B", "/b.ts"), file_id("E", "/b.ts"));
    assert_ne!(file_id("B", "/a.ts"), file_id("B", "/b.ts"));
    let check = events
        .iter()
        .find(|event| event.ph == "B" && event.name == "checkSourceFile")
        .unwrap();
    let variance = events
        .iter()
        .find(|event| event.ph == "B" && event.name == "getVariancesWorker")
        .unwrap();
    assert_eq!(variance.argument::<i64>("id"), Some(1));
    assert_eq!(check.tid, variance.tid);
    for (tid, name) in [
        (file_id("B", "/a.ts"), "file:/a.ts"),
        (file_id("B", "/b.ts"), "file:/b.ts"),
        (check.tid, "checker:0"),
    ] {
        assert!(events.iter().any(|event| event.ph == "M"
            && event.name == "thread_name"
            && event.tid == tid
            && event.argument::<String>("name").as_deref() == Some(name)));
    }
    let mut stacks: BTreeMap<i64, Vec<(&str, &str)>> = BTreeMap::new();
    for event in &events {
        match event.ph.as_str() {
            "B" => stacks
                .entry(event.tid)
                .or_default()
                .push((&event.cat, &event.name)),
            "E" => assert_eq!(
                stacks.entry(event.tid).or_default().pop(),
                Some((event.cat.as_str(), event.name.as_str()))
            ),
            _ => {}
        }
    }
    assert!(stacks.values().all(Vec::is_empty));
}

#[test]
// source: tsc/internal/tracing/tracing_test.go:TestThreadIDsAreStableAcrossFirstSeenOrder
fn thread_ids_are_stable_across_first_seen_order_in_written_events() {
    let collect = |paths: [&str; 2]| {
        let fs = fs();
        let tr = start_tracing(fs.clone(), b"/trace", b"", true).unwrap();
        for path in paths {
            drop(tr.span(TracePhase::Parse, "createSourceFile", args(path), true));
        }
        stop(&tr);
        observe(fs.as_ref())
            .into_iter()
            .filter(|event| event.ph == "B" && event.name == "createSourceFile")
            .map(|event| (event.argument::<String>("path").unwrap(), event.tid))
            .collect::<BTreeMap<_, _>>()
    };
    assert_eq!(collect(["/a.ts", "/b.ts"]), collect(["/b.ts", "/a.ts"]));
}

#[test]
fn deterministic_sampling_skips_clock_and_complete_events_capture_start_args() {
    let tr = start_tracing(fs(), b"/trace", b"", true).unwrap();
    assert_eq!(
        tr.push(TracePhase::CheckTypes, "sampled", &args("/a.ts"), false),
        0
    );
    assert_eq!(lock(&tr.state).counter, 1);
    stop(&tr);
    let fs = fs();
    let tr = start_tracing(fs.clone(), b"/trace", b"", false).unwrap();
    let token = tr.push(TracePhase::CheckTypes, "sampled", &args("/start.ts"), false);
    std::thread::sleep(std::time::Duration::from_millis(12));
    tr.pop(token, &args("/changed.ts"));
    stop(&tr);
    let trace = text(fs.as_ref(), b"/trace/trace.json");
    assert!(trace.contains("\"ph\":\"X\""));
    assert!(trace.contains("\"dur\":"));
    assert!(trace.contains("\"path\":\"/start.ts\""));
    assert!(!trace.contains("/changed.ts"));
}

#[test]
fn resolution_boolean_arguments_preserve_json_type() {
    let fs = fs();
    let tr = start_tracing(fs.clone(), b"/trace", b"", true).unwrap();
    tr.instant(
        TracePhase::Program,
        "resolution",
        &[
            ("hasResolved".into(), TraceValue::Bool(false)),
            ("success".into(), TraceValue::Bool(true)),
        ]
        .into_iter()
        .collect(),
    );
    stop(&tr);
    assert!(text(fs.as_ref(), b"/trace/trace.json")
        .contains("\"args\":{\"hasResolved\":false,\"success\":true}"));
}

#[test]
fn full_recorded_type_snapshot_is_resolved_without_holding_session_lock() {
    let fs = fs();
    let tr = start_tracing(fs.clone(), b"/trace", b"/project/tsconfig.json", true).unwrap();
    tr.new_type_tracer(2); // Empty tracers still appear in the legend.
    tr.record_type(10, 1);
    tr.record_type(10, 2);
    let mut calls = Vec::new();
    tr.stop_tracing(|index, ids| {
        calls.push((index, ids.to_vec()));
        tr.instant(TracePhase::CheckTypes, "display", &TraceArgs::new());
        tr.record_type(index, 3); // Display-created types are outside this snapshot.
        Ok(ids
            .iter()
            .map(|id| TraceTypeRecord {
                id: *id,
                flags: vec!["Number"],
                ..Default::default()
            })
            .collect())
    })
    .unwrap();
    assert_eq!(calls, [(10, vec![1, 2])]);
    assert_eq!(
        text(fs.as_ref(), b"/trace/types_10.json"),
        "[{\"id\":1,\"flags\":[\"Number\"]},\n{\"id\":2,\"flags\":[\"Number\"]}]\n"
    );
    assert!(fs.read_file(b"/trace/types_2.json").unwrap().is_none());
    let legend = text(fs.as_ref(), b"/trace/legend.json");
    assert!(legend.find("types_10.json").unwrap() < legend.find("types_2.json").unwrap());
    assert!(legend.contains("\"configFilePath\": \"/project/tsconfig.json\""));
    let before = text(fs.as_ref(), b"/trace/trace.json");
    tr.instant(TracePhase::Program, "after-stop", &TraceArgs::new());
    assert_eq!(lock(&tr.state).buffer.len(), 0);
    assert_eq!(text(fs.as_ref(), b"/trace/trace.json"), before);
}

struct FailingFs {
    inner: Arc<dyn FileSystem>,
    fail: AtomicBool,
    appends: AtomicUsize,
}
impl FileSystem for FailingFs {
    fn use_case_sensitive_file_names(&self) -> bool {
        true
    }
    fn snapshot_id(&self) -> Option<SnapshotId> {
        None
    }
    fn read_file(&self, p: &[u8]) -> Result<Option<FileContent>, tsr_vfs::Error> {
        self.inner.read_file(p)
    }
    fn stat(&self, p: &[u8]) -> Result<Option<FileInfo>, tsr_vfs::Error> {
        self.inner.stat(p)
    }
    fn entries(&self, p: &[u8]) -> Result<Entries, tsr_vfs::Error> {
        self.inner.entries(p)
    }
    fn realpath(&self, p: &[u8]) -> Result<JsString, tsr_vfs::Error> {
        self.inner.realpath(p)
    }
    fn write_file(&self, p: &[u8], data: &[u8]) -> Result<(), tsr_vfs::Error> {
        self.inner.write_file(p, data)
    }
    fn append_file(&self, p: &[u8], data: &[u8]) -> Result<(), tsr_vfs::Error> {
        self.appends.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            Err(tsr_vfs::Error::Io(std::io::ErrorKind::StorageFull))
        } else {
            self.inner.append_file(p, data)
        }
    }
}

#[test]
fn first_flush_error_is_reported_once_and_stops_buffer_growth() {
    let fs = Arc::new(FailingFs {
        inner: fs(),
        fail: AtomicBool::new(true),
        appends: AtomicUsize::new(0),
    });
    let tr = start_tracing(fs.clone(), b"/trace", b"", true).unwrap();
    let args = [("data".into(), TraceValue::Str("x".repeat(FLUSH_THRESHOLD)))]
        .into_iter()
        .collect();
    tr.instant(TracePhase::Program, "large", &args);
    tr.instant(TracePhase::Program, "large-again", &args);
    assert_eq!(fs.appends.load(Ordering::SeqCst), 1);
    assert_eq!(lock(&tr.state).buffer.len(), 0);
    assert!(tr
        .stop_tracing(|_, _| unreachable!())
        .unwrap_err()
        .to_string()
        .starts_with("failed to flush trace file:"));
    tr.instant(TracePhase::Program, "late", &args);
    assert_eq!(lock(&tr.state).buffer.len(), 0);
}

#[test]
fn final_append_failure_can_retry_without_duplicate_closing_bracket() {
    let fs = Arc::new(FailingFs {
        inner: fs(),
        fail: AtomicBool::new(true),
        appends: AtomicUsize::new(0),
    });
    let tr = start_tracing(fs.clone(), b"/trace", b"", true).unwrap();
    tr.instant(TracePhase::Program, "small", &TraceArgs::new());
    assert!(tr
        .stop_tracing(|_, _| unreachable!())
        .unwrap_err()
        .to_string()
        .starts_with("failed to write trace file:"));
    fs.fail.store(false, Ordering::SeqCst);
    stop(&tr);
    let trace = text(fs.as_ref(), b"/trace/trace.json");
    assert_eq!(trace.matches("\n]\n").count(), 1);
    assert_eq!(trace.matches("\"name\":\"small\"").count(), 1);
}
