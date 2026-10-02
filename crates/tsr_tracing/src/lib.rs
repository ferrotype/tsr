//! File-backed tracing through the production checker trace boundary.
//! Type IDs are resolved while their checker is held by the caller at stop;
//! this session never owns a partial or detached replacement type table.
#[cfg(test)]
mod tests;
mod wire;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;
use tsr_checker::{TraceArgs, TracePhase, TraceSink, TraceTypeRecord, TraceValue};
use tsr_jsstring::JsString;
use tsr_vfs::FileSystem;
use wire::{Event, TraceRecord};

const FLUSH_THRESHOLD: usize = 256 * 1024;
const SAMPLE_MICROS: f64 = 10_000.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(String);
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ThreadKey {
    Checker(i64),
    File(String),
}
impl ThreadKey {
    // port: tsc/internal/tracing/tracing.go:traceThreadKeyFromArgs
    fn from_args(args: &TraceArgs) -> Option<Self> {
        if let Some(TraceValue::Int(index)) = args.get("checkerId") {
            return Some(Self::Checker(*index));
        }
        for key in [
            "path",
            "fileName",
            "containingFileName",
            "jsFilePath",
            "declarationFilePath",
        ] {
            if let Some(TraceValue::Str(path)) = args.get(key) {
                if !path.is_empty() {
                    return Some(Self::File(path.clone()));
                }
            }
        }
        None
    }
    // port: tsc/internal/tracing/tracing.go:traceThreadKey.displayName
    fn name(&self) -> String {
        match self {
            Self::Checker(index) => format!("checker:{index}"),
            Self::File(path) => format!("file:{path}"),
        }
    }
    // port: tsc/internal/tracing/tracing.go:traceThreadKey.defaultThreadID
    fn default_id(&self) -> i64 {
        if let Self::Checker(index) = self {
            if *index >= 0 {
                return index.wrapping_add(2);
            }
        }
        // port: tsc/internal/tracing/tracing.go:stableTraceThreadID
        1_000_000 + (xxhash_rust::xxh3::xxh3_64(self.name().as_bytes()) % 1_000_000_000) as i64
    }
}

struct OpenEvent {
    phase: TracePhase,
    name: String,
    args: TraceArgs,
    separate: bool,
    tid: i64,
    started: Instant,
}
struct TypeTracer {
    checker_index: usize,
    ids: Vec<u32>,
}
struct State {
    started: bool,
    buffer: Vec<u8>,
    counter: u64,
    metadata_ts: f64,
    thread_ids: BTreeMap<ThreadKey, i64>,
    thread_keys: BTreeMap<i64, ThreadKey>,
    next_token: u64,
    open: BTreeMap<u64, OpenEvent>,
    tracers: Vec<TypeTracer>,
    legend: Vec<TraceRecord>,
    flush_error: Option<Error>,
}

pub struct Tracing {
    fs: Arc<dyn FileSystem>,
    trace_dir: Vec<u8>,
    trace_path: Vec<u8>,
    config_file_path: JsString,
    deterministic: bool,
    started_at: Instant,
    state: Mutex<State>,
}

/// port: tsc/internal/tracing/tracing.go:StartTracing
pub fn start_tracing(
    fs: Arc<dyn FileSystem>,
    trace_dir: &[u8],
    config_file_path: &[u8],
    deterministic: bool,
) -> Result<Arc<Tracing>, Error> {
    let trace_path = tsr_tspath::combine(trace_dir, &[b"trace.json"]);
    let started_at = Instant::now();
    let mut state = State {
        started: true,
        buffer: b"[\n".to_vec(),
        counter: 0,
        metadata_ts: 0.0,
        thread_ids: BTreeMap::new(),
        thread_keys: BTreeMap::new(),
        next_token: 1,
        open: BTreeMap::new(),
        tracers: Vec::new(),
        legend: Vec::new(),
        flush_error: None,
    };
    let ts = if deterministic {
        state.counter = 1;
        1.0
    } else {
        started_at.elapsed().as_nanos() as f64 / 1000.0
    };
    state.metadata_ts = ts;
    let mut args = TraceArgs::new();
    args.insert("name".into(), TraceValue::Str("tsgo".into()));
    wire::write_event(
        &mut state.buffer,
        &Event::metadata(1, ts, "process_name", Some(&args)),
    );
    state.buffer.extend_from_slice(b",\n");
    args.insert("name".into(), TraceValue::Str("Main".into()));
    wire::write_event(
        &mut state.buffer,
        &Event::metadata(1, ts, "thread_name", Some(&args)),
    );
    state.buffer.extend_from_slice(b",\n");
    wire::write_event(
        &mut state.buffer,
        &Event {
            tid: 1,
            ph: "M",
            category: "disabled-by-default-devtools.timeline",
            ts,
            name: "TracingStartedInBrowser",
            scope: "",
            duration: None,
            args: None,
        },
    );
    fs.write_file(&trace_path, &state.buffer)
        .map_err(|error| Error(format!("failed to write trace file header: {error}")))?;
    state.buffer.clear();
    Ok(Arc::new(Tracing {
        fs,
        trace_dir: trace_dir.to_vec(),
        trace_path,
        config_file_path: JsString::from_bytes(config_file_path),
        deterministic,
        started_at,
        state: Mutex::new(state),
    }))
}

impl Tracing {
    // port: tsc/internal/tracing/tracing.go:Tracing.timestamp
    fn timestamp(&self, state: &mut State) -> f64 {
        if self.deterministic {
            state.counter = state.counter.wrapping_add(1);
            state.counter as f64
        } else {
            self.started_at.elapsed().as_nanos() as f64 / 1000.0
        }
    }
    // port: tsc/internal/tracing/tracing.go:Tracing.threadIDLocked
    fn thread_id(state: &mut State, args: &TraceArgs) -> i64 {
        let Some(key) = ThreadKey::from_args(args) else {
            return 1;
        };
        if let Some(id) = state.thread_ids.get(&key) {
            return *id;
        }
        let mut id = key.default_id();
        while state
            .thread_keys
            .get(&id)
            .is_some_and(|existing| existing != &key)
        {
            id = id.wrapping_add(1);
        }
        // port: tsc/internal/tracing/tracing.go:Tracing.writeThreadNameEventLocked
        let mut args = TraceArgs::new();
        args.insert("name".into(), TraceValue::Str(key.name()));
        state.thread_keys.insert(id, key.clone());
        state.thread_ids.insert(key, id);
        state.buffer.extend_from_slice(b",\n");
        wire::write_event(
            &mut state.buffer,
            &Event::metadata(id, state.metadata_ts, "thread_name", Some(&args)),
        );
        id
    }
    // port: tsc/internal/tracing/tracing.go:Tracing.maybeFlushLocked
    fn maybe_flush(&self, state: &mut State) {
        if state.flush_error.is_some() {
            state.buffer.clear();
            return;
        }
        if state.buffer.len() < FLUSH_THRESHOLD {
            return;
        }
        if let Err(error) = self.fs.append_file(&self.trace_path, &state.buffer) {
            state.flush_error = Some(Error(format!("failed to flush trace file: {error}")));
        }
        state.buffer.clear();
    }
    fn ensure_type_tracer(&self, state: &mut State, checker_index: usize) -> usize {
        if let Some(index) = state
            .tracers
            .iter()
            .position(|tracer| tracer.checker_index == checker_index)
        {
            return index;
        }
        let types_path = tsr_tspath::combine(
            &self.trace_dir,
            &[format!("types_{checker_index}.json").as_bytes()],
        );
        state.legend.push(TraceRecord {
            config_file_path: self.config_file_path.clone(),
            trace_path: JsString::from_bytes(self.trace_path.as_slice()),
            types_path: JsString::from_bytes(types_path),
            checker_id: checker_index,
        });
        state.tracers.push(TypeTracer {
            checker_index,
            ids: Vec::new(),
        });
        state.tracers.len() - 1
    }
    /// Stop resolves every recorded ID through its original checker. The callback
    /// runs without the session mutex: formatting a type may emit events or create
    /// additional types, exactly as the pin's DumpTypes does.
    /// port: tsc/internal/tracing/tracing.go:Tracing.StopTracing
    pub fn stop_tracing(
        &self,
        mut describe: impl FnMut(usize, &[u32]) -> Result<Vec<TraceTypeRecord>, String>,
    ) -> Result<(), Error> {
        let checker_indices: Vec<_> = lock(&self.state)
            .tracers
            .iter()
            .map(|tracer| tracer.checker_index)
            .collect();
        for checker_index in checker_indices {
            // port: tsc/internal/tracing/tracing.go:typeTracer.DumpTypes
            let ids = lock(&self.state)
                .tracers
                .iter()
                .find(|tracer| tracer.checker_index == checker_index)
                .expect("registered type tracer remains present")
                .ids
                .clone();
            if ids.is_empty() {
                continue;
            }
            let records = describe(checker_index, &ids).map_err(|error| {
                Error(format!(
                    "failed to dump types for checker {checker_index}: {error}"
                ))
            })?;
            let mut bytes = Vec::new();
            tsr_checker::write_type_records(&mut bytes, &records).map_err(|error| {
                Error(format!(
                    "failed to dump types for checker {checker_index}: {error}"
                ))
            })?;
            let path = tsr_tspath::combine(
                &self.trace_dir,
                &[format!("types_{checker_index}.json").as_bytes()],
            );
            self.fs.write_file(&path, &bytes).map_err(|error| {
                Error(format!(
                    "failed to dump types for checker {checker_index}: {error}"
                ))
            })?;
        }
        let mut state = lock(&self.state);
        if state.started {
            if let Some(error) = state.flush_error.clone() {
                state.buffer.clear();
                state.started = false;
                state.open.clear();
                return Err(error);
            }
            let mut final_bytes = state.buffer.clone();
            final_bytes.extend_from_slice(b"\n]\n");
            self.fs
                .append_file(&self.trace_path, &final_bytes)
                .map_err(|error| Error(format!("failed to write trace file: {error}")))?;
            state.buffer.clear();
            state.started = false;
            state.open.clear();
        }
        state
            .legend
            .sort_by(|left, right| left.types_path.cmp(&right.types_path));
        let bytes = tsr_json::marshal_indent(&state.legend, "", "  ")
            .map_err(|error| Error(format!("failed to marshal legend file: {error}")))?;
        let path = tsr_tspath::combine(&self.trace_dir, &[b"legend.json"]);
        self.fs
            .write_file(&path, &bytes)
            .map_err(|error| Error(format!("failed to write legend file: {error}")))
    }
    pub fn span(
        self: &Arc<Self>,
        phase: TracePhase,
        name: &str,
        args: TraceArgs,
        separate: bool,
    ) -> Span {
        let token = self.push(phase, name, &args, separate);
        Span {
            tracing: self.clone(),
            token,
            args,
        }
    }
}

#[must_use]
pub struct Span {
    tracing: Arc<Tracing>,
    token: u64,
    args: TraceArgs,
}
impl Span {
    pub fn args_mut(&mut self) -> &mut TraceArgs {
        &mut self.args
    }
}
impl Drop for Span {
    fn drop(&mut self) {
        self.tracing.pop(self.token, &self.args);
    }
}

impl TraceSink for Tracing {
    // The returned Go closure is TraceSink::pop below; both halves retain the
    // same span state and run at the actual operation boundaries.
    // port: tsc/internal/tracing/tracing.go:Tracing.Push
    fn push(&self, phase: TracePhase, name: &str, args: &TraceArgs, separate: bool) -> u64 {
        if !separate && self.deterministic {
            return 0;
        }
        let mut state = lock(&self.state);
        if !state.started {
            return 0;
        }
        let started = Instant::now();
        let tid = if separate {
            let ts = self.timestamp(&mut state);
            let tid = Self::thread_id(&mut state, args);
            state.buffer.extend_from_slice(b",\n");
            wire::write_event(
                &mut state.buffer,
                &Event {
                    tid,
                    ph: "B",
                    category: phase.as_str(),
                    ts,
                    name,
                    scope: "",
                    duration: None,
                    args: Some(args),
                },
            );
            self.maybe_flush(&mut state);
            tid
        } else {
            0
        };
        let token = state.next_token;
        state.next_token = state
            .next_token
            .checked_add(1)
            .expect("trace token space exhausted");
        state.open.insert(
            token,
            OpenEvent {
                phase,
                name: name.into(),
                args: if separate {
                    TraceArgs::new()
                } else {
                    args.clone()
                },
                separate,
                tid,
                started,
            },
        );
        token
    }
    fn pop(&self, token: u64, args: &TraceArgs) {
        if token == 0 {
            return;
        }
        let mut state = lock(&self.state);
        let Some(open) = state.open.remove(&token) else {
            return;
        };
        if !state.started {
            return;
        }
        let (tid, ts, duration, args) = if open.separate {
            (open.tid, self.timestamp(&mut state), None, args)
        } else {
            let duration = open.started.elapsed().as_nanos() as f64 / 1000.0;
            let start = open.started.duration_since(self.started_at).as_nanos() as f64 / 1000.0;
            if SAMPLE_MICROS - start % SAMPLE_MICROS > duration {
                return;
            }
            (
                Self::thread_id(&mut state, &open.args),
                start,
                Some(duration),
                &open.args,
            )
        };
        state.buffer.extend_from_slice(b",\n");
        wire::write_event(
            &mut state.buffer,
            &Event {
                tid,
                ph: if open.separate { "E" } else { "X" },
                category: open.phase.as_str(),
                ts,
                name: &open.name,
                scope: "",
                duration,
                args: Some(args),
            },
        );
        self.maybe_flush(&mut state);
    }
    // port: tsc/internal/tracing/tracing.go:Tracing.Instant
    fn instant(&self, phase: TracePhase, name: &str, args: &TraceArgs) {
        let mut state = lock(&self.state);
        if !state.started {
            return;
        }
        let ts = self.timestamp(&mut state);
        let tid = Self::thread_id(&mut state, args);
        state.buffer.extend_from_slice(b",\n");
        wire::write_event(
            &mut state.buffer,
            &Event {
                tid,
                ph: "I",
                category: phase.as_str(),
                ts,
                name,
                scope: "g",
                duration: None,
                args: Some(args),
            },
        );
        self.maybe_flush(&mut state);
    }
    // port: tsc/internal/tracing/tracing.go:Tracing.NewTypeTracer
    fn new_type_tracer(&self, checker_index: usize) {
        self.ensure_type_tracer(&mut lock(&self.state), checker_index);
    }
    // port: tsc/internal/tracing/tracing.go:typeTracer.RecordType
    fn record_type(&self, checker_index: usize, type_id: u32) {
        let mut state = lock(&self.state);
        let index = self.ensure_type_tracer(&mut state, checker_index);
        state.tracers[index].ids.push(type_id);
    }
}
