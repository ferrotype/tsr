//! The trace and kill protocol of the Phase 1 mutation oracles.
//!
//! Shared by the mutation driver (the `e1`, `binder` and `facts` oracles) and
//! by the `phase1_syntax` harness's mutation mode (the `syntax` oracle), which
//! includes this file with `#[path]`. An oracle supplies request parsing and
//! one row's execution through [`Rows`]; this module owns the row loop, the
//! mutant switch state per stage, reach recording, kill jobs, control mutants
//! and base rechecks, so every oracle speaks the same protocol to
//! `scripts/phase1_mutation_run.py`.
//!
//! Reach and activation follow the contract's stage rule. A row starts in the
//! production stage with its mutant active; [`enter`] switches stages. Hits
//! recorded while producing are the row's reach (`hits`). Parser-site hits
//! recorded while observing count as reach, and the mutant stays live, only for
//! oracles whose observation runs production parser work (E1's lazy JSDoc); for
//! every other oracle the mutant is switched off while observing. Every site
//! executed while observing that does not count as reach, including every
//! production-only (`hit`) site, which never activates there, is reported as
//! `observe_hits`: observation reach, which the supervisor needs before it
//! calls a home unexecuted.
//!
//! `trace` writes one line per row: `{"row","request_sha256","outcomes",
//! "digests","hits","micros","source_bytes"}` plus `messages` (failed stages),
//! `error` (session defects) and `observe_hits` when present.
//!
//! `kill` prints `{"ready":true,...}`, then reads jobs `{"mutant":id,"rows":
//! [..],"max_kills":3}` (optional `control`, `report_all`, `dump_all`,
//! `recheck`) from stdin. Every row run is preceded by a heartbeat
//! `{"heartbeat":row,"mutant":id}` (with `"control":id`, `"dump":true` or
//! `"recheck":true` for the extra runs). A row whose compared digests differ
//! from the base, that crashed, or that `report_all` asks for prints
//! `{"mutant","row","outcomes","digests","differs","crash","kill","dump",
//! "micros"}`. A differing, non-crashing row of a mutant with a control runs
//! the control on the same row; it is a kill only when the control completes
//! like the base and, in some compared stage where the mutant differs from the
//! base, the mutant also differs from the control (`"control":{"id",
//! "outcomes","digests","differs","crash","dump"}`, `differs` naming every
//! stage where mutant and control differ). Kill rows are rerun with no mutant afterwards
//! (`{"mutant","recheck":row,"base_ok"}`) and the job closes with
//! `{"mutant","done":true,"ran","differing","crashes"}`, `differing` counting
//! kills. Every line is flushed.

use phase1_mutants::{
    begin_row, set_active, set_stage, set_tracing, stage, take_row_hits, take_row_observe_hits,
};
pub use phase1_mutants::{MutantId, Stage};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufRead, BufWriter, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::time::Instant;

/// One authenticated request row, in inventory order.
pub struct Request {
    pub index: usize,
    pub id: String,
    pub sha256: String,
    pub value: Value,
    pub source_bytes: usize,
}

/// What one row produced, in the row shape of `phase1_mutation_go.finish_row`.
#[derive(Debug, Default)]
pub struct RowOutput {
    /// Every operation's outcome (`ok`, `error`, `panic`, `not_run`, ...).
    pub outcomes: BTreeMap<String, String>,
    /// The message of every operation whose outcome is not `ok`, hex encoded.
    pub messages: BTreeMap<String, String>,
    /// sha256 per compared stage that completed.
    pub digests: BTreeMap<String, String>,
    /// The row's frames, when captured.
    pub frames: Option<Vec<u8>>,
    /// A session defect: serialization, oversize frame, broken fragment.
    pub error: Option<String>,
}

/// A row run: its output, its reach and how long it took.
pub struct Executed {
    pub output: RowOutput,
    /// Sites reached while producing (and, where observation counts, parser
    /// sites reached while observing).
    pub hits: Vec<MutantId>,
    /// Every other site executed while observing: observation reach.
    pub observe_hits: Vec<MutantId>,
    pub micros: u64,
}

/// An oracle's rows.
pub trait Rows {
    fn name(&self) -> &'static str;
    /// The stages whose digests are compared, in protocol order.
    fn compared(&self) -> &'static [&'static str];
    /// Whether mutants stay live, and their hits count as reach, while
    /// observing (E1: lazy JSDoc parsing during encoding is production work).
    fn observe_counts(&self) -> bool;
    /// Whether a differing row is run again to dump its frames for review
    /// and the supervisor's independent digest check.
    fn dumps_kills(&self) -> bool;
    /// Parses, authenticates and validates the request on one input line.
    fn request(&self, index: usize, line: &[u8]) -> Result<Request, String>;
    /// Runs one row on this thread. Must not unwind: stage panics are outcomes.
    fn execute(&self, request: &Request, capture: bool) -> RowOutput;
}

struct RowReach {
    mutant: MutantId,
    observe_counts: bool,
    production: Vec<MutantId>,
    observe: Vec<MutantId>,
}

impl RowReach {
    /// Files the hits recorded since the last switch under the stage that
    /// recorded them; production-only sites executed while observing are
    /// always observation reach.
    fn file(&mut self, recorded_in: Stage) {
        let hits = take_row_hits();
        if recorded_in == Stage::Production || self.observe_counts {
            self.production.extend(hits);
        } else {
            self.observe.extend(hits);
        }
        self.observe.extend(take_row_observe_hits());
    }
}

thread_local! {
    static ROW: RefCell<Option<RowReach>> = const { RefCell::new(None) };
}

/// Switches this thread to `next`. Inside a mutation row it also files the
/// reach recorded so far and, for oracles whose observation counts nothing,
/// switches the mutant off while observing. Outside a row it only sets the
/// stage, so evidence runs are unaffected.
pub fn enter(next: Stage) {
    ROW.with(|row| {
        if let Some(row) = row.borrow_mut().as_mut() {
            let current = stage();
            if current != next {
                row.file(current);
                let live = next == Stage::Production || row.observe_counts;
                set_active(if live { row.mutant } else { 0 });
            }
        }
    });
    set_stage(next);
}

fn sorted(mut ids: Vec<MutantId>) -> Vec<MutantId> {
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// Runs one row with `mutant` active (0: none) and reach recorded from a clean
/// slate; the thread ends in the production stage with no active mutant.
pub fn run_row(rows: &impl Rows, request: &Request, capture: bool, mutant: MutantId) -> Executed {
    set_stage(Stage::Production);
    begin_row();
    set_active(mutant);
    ROW.with(|row| {
        *row.borrow_mut() = Some(RowReach {
            mutant,
            observe_counts: rows.observe_counts(),
            production: Vec::new(),
            observe: Vec::new(),
        });
    });
    let started = Instant::now();
    let escaped = catch_unwind(AssertUnwindSafe(|| rows.execute(request, capture)));
    let micros = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
    let reach = ROW.with(|row| row.borrow_mut().take()).map(|mut reach| {
        reach.file(stage());
        reach
    });
    set_active(0);
    set_stage(Stage::Production);
    let output = escaped.unwrap_or_else(|payload| RowOutput {
        error: Some(format!("row panic: {}", panic_message(payload.as_ref()))),
        ..RowOutput::default()
    });
    let (hits, observe_hits) = reach.map_or_else(Default::default, |reach| {
        (sorted(reach.production), sorted(reach.observe))
    });
    Executed {
        output,
        hits,
        observe_hits,
        micros,
    }
}

pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload.downcast_ref::<&str>().map_or_else(
        || {
            payload
                .downcast_ref::<String>()
                .cloned()
                .unwrap_or_else(|| "non-string panic payload".into())
        },
        |message| (*message).into(),
    )
}

pub fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 15)]));
    }
    out
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    lower_hex(&Sha256::digest(bytes))
}

/// Loads every request line through the oracle, refusing duplicate ids.
pub fn load_requests(rows: &impl Rows, path: &Path) -> Result<Vec<Request>, String> {
    let bytes = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut requests: Vec<Request> = Vec::new();
    let mut seen = HashSet::new();
    for (line_number, line) in bytes.split(|&byte| byte == b'\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        let request = rows
            .request(requests.len(), line)
            .map_err(|error| format!("{}:{}: {error}", path.display(), line_number + 1))?;
        if !seen.insert(request.id.clone()) {
            return Err(format!(
                "{}:{}: duplicate request {}",
                path.display(),
                line_number + 1,
                request.id
            ));
        }
        requests.push(request);
    }
    Ok(requests)
}

/// Outcomes, digests and the optional failure fields of one row.
pub fn row_fields(output: &RowOutput) -> Map<String, Value> {
    let mut fields = Map::new();
    fields.insert("outcomes".into(), json!(output.outcomes));
    fields.insert("digests".into(), json!(output.digests));
    if !output.messages.is_empty() {
        fields.insert("messages".into(), json!(output.messages));
    }
    if let Some(error) = &output.error {
        fields.insert("error".into(), json!(error));
    }
    fields
}

fn write_dump(dir: &Path, name: &str, frames: &[u8]) -> Result<String, String> {
    let path = dir.join(name);
    fs::write(&path, frames).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(path.display().to_string())
}

fn emit(out: &mut impl Write, value: &Value) -> Result<(), String> {
    serde_json::to_writer(&mut *out, value).map_err(|error| error.to_string())?;
    out.write_all(b"\n")
        .and_then(|()| out.flush())
        .map_err(|error| error.to_string())
}

/// Every row in order, with no mutant and reach recorded.
pub fn trace(
    rows: &impl Rows,
    requests: &[Request],
    out: &Path,
    frames: Option<&Path>,
    dump_dir: Option<&Path>,
) -> Result<(), String> {
    let io_error = |path: &Path, error: io::Error| format!("{}: {error}", path.display());
    let mut out_file = BufWriter::new(File::create(out).map_err(|error| io_error(out, error))?);
    let mut frames_file = frames
        .map(|path| {
            File::create(path)
                .map(|file| (path, BufWriter::new(file)))
                .map_err(|error| io_error(path, error))
        })
        .transpose()?;
    if let Some(dir) = dump_dir {
        fs::create_dir_all(dir).map_err(|error| io_error(dir, error))?;
    }
    set_active(0);
    set_tracing(true);
    let capture = frames_file.is_some() || dump_dir.is_some();
    for request in requests {
        let executed = run_row(rows, request, capture, 0);
        if let Some(captured) = &executed.output.frames {
            if let Some((path, file)) = frames_file.as_mut() {
                file.write_all(captured)
                    .map_err(|error| io_error(path, error))?;
            }
            if let Some(dir) = dump_dir {
                write_dump(dir, &format!("trace-r{}.ndjson", request.index), captured)?;
            }
        }
        let mut line = row_fields(&executed.output);
        line.insert("row".into(), json!(request.id));
        line.insert("request_sha256".into(), json!(request.sha256));
        line.insert("hits".into(), json!(executed.hits));
        if !executed.observe_hits.is_empty() {
            line.insert("observe_hits".into(), json!(executed.observe_hits));
        }
        line.insert("micros".into(), json!(executed.micros));
        line.insert("source_bytes".into(), json!(request.source_bytes));
        serde_json::to_writer(&mut out_file, &Value::Object(line))
            .map_err(|error| error.to_string())?;
        out_file
            .write_all(b"\n")
            .map_err(|error| io_error(out, error))?;
    }
    out_file.flush().map_err(|error| io_error(out, error))?;
    if let Some((path, file)) = frames_file.as_mut() {
        file.flush().map_err(|error| io_error(path, error))?;
    }
    Ok(())
}

/// A base row as the supervisor writes it: what a row with no mutant produces.
pub struct BaseRow {
    pub outcomes: BTreeMap<String, String>,
    pub digests: BTreeMap<String, String>,
}

fn string_map(value: &Value, field: &str) -> Result<BTreeMap<String, String>, String> {
    value[field]
        .as_object()
        .ok_or_else(|| format!("base row without {field}"))?
        .iter()
        .map(|(key, value)| {
            value
                .as_str()
                .map(|text| (key.clone(), text.to_owned()))
                .ok_or_else(|| format!("base row {field}.{key} is not a string"))
        })
        .collect()
}

fn load_base(path: &Path, requests: &[Request]) -> Result<HashMap<String, BaseRow>, String> {
    let by_id: HashMap<&str, &Request> = requests
        .iter()
        .map(|request| (request.id.as_str(), request))
        .collect();
    let bytes = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut base = HashMap::new();
    for line in bytes.split(|&byte| byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        let value: Value =
            serde_json::from_slice(line).map_err(|error| format!("{}: {error}", path.display()))?;
        let row = value["row"].as_str().ok_or("base row without row")?;
        let request = by_id
            .get(row)
            .ok_or_else(|| format!("base row {row} is not a request"))?;
        if value["request_sha256"].as_str() != Some(request.sha256.as_str()) {
            return Err(format!("base row {row} has another request_sha256"));
        }
        base.insert(
            row.to_owned(),
            BaseRow {
                outcomes: string_map(&value, "outcomes")?,
                digests: string_map(&value, "digests")?,
            },
        );
    }
    Ok(base)
}

/// One kill job, as the supervisor sends it.
pub struct Job {
    pub mutant: MutantId,
    pub rows: Vec<String>,
    /// Stop after this many kills; 0 runs every row.
    pub max_kills: usize,
    /// The paired control mutant of an allocating operator.
    pub control: Option<MutantId>,
    /// Print every row's result, not only differing or crashing rows.
    pub report_all: bool,
    /// Write every row's frames to the dump directory.
    pub dump_all: bool,
    /// Rerun each kill row with no active mutant afterwards.
    pub recheck: bool,
}

fn mutant_id(value: &Value, field: &str) -> Result<MutantId, String> {
    value
        .as_u64()
        .and_then(|id| MutantId::try_from(id).ok())
        .ok_or_else(|| format!("job {field} is not a mutant id"))
}

fn flag(object: &Map<String, Value>, field: &str, default: bool) -> Result<bool, String> {
    object.get(field).map_or(Ok(default), |value| {
        value
            .as_bool()
            .ok_or_else(|| format!("job {field} is not a boolean"))
    })
}

impl Job {
    pub fn parse(line: &str) -> Result<Self, String> {
        let value: Value =
            serde_json::from_str(line).map_err(|error| format!("invalid job: {error}"))?;
        let object = value.as_object().ok_or("a job is a JSON object")?;
        let known = [
            "mutant",
            "rows",
            "max_kills",
            "control",
            "report_all",
            "dump_all",
            "recheck",
        ];
        if let Some(unknown) = object.keys().find(|key| !known.contains(&key.as_str())) {
            return Err(format!("unknown job field {unknown:?}"));
        }
        let rows = object
            .get("rows")
            .and_then(Value::as_array)
            .ok_or("job rows is not a list")?
            .iter()
            .map(|row| {
                row.as_str()
                    .map(str::to_owned)
                    .ok_or("job row is not a string")
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            mutant: mutant_id(object.get("mutant").unwrap_or(&Value::Null), "mutant")?,
            rows,
            max_kills: object.get("max_kills").map_or(Ok(3), |value| {
                value
                    .as_u64()
                    .and_then(|count| usize::try_from(count).ok())
                    .ok_or("job max_kills is not a count")
            })?,
            control: match object.get("control") {
                None | Some(Value::Null) => None,
                Some(value) => Some(mutant_id(value, "control")?),
            },
            report_all: flag(object, "report_all", false)?,
            dump_all: flag(object, "dump_all", false)?,
            recheck: flag(object, "recheck", true)?,
        })
    }
}

/// Which compared stages differ between two digest maps.
fn differing(
    compared: &[&str],
    left: &BTreeMap<String, String>,
    right: &BTreeMap<String, String>,
) -> Vec<String> {
    compared
        .iter()
        .filter(|stage| left.get(**stage) != right.get(**stage))
        .map(|stage| (*stage).to_owned())
        .collect()
}

/// Any outcome other than the base's, or a session defect, is a crash.
fn crashed(base: &BaseRow, output: &RowOutput) -> bool {
    output.outcomes != base.outcomes || output.error.is_some()
}

/// Reads jobs from stdin and runs them against the base rows.
pub fn kill(
    rows: &impl Rows,
    requests: &[Request],
    base_path: &Path,
    dump_dir: &Path,
) -> Result<(), String> {
    fs::create_dir_all(dump_dir).map_err(|error| format!("{}: {error}", dump_dir.display()))?;
    let base = load_base(base_path, requests)?;
    let by_id: HashMap<&str, &Request> = requests
        .iter()
        .map(|request| (request.id.as_str(), request))
        .collect();
    set_active(0);
    set_tracing(false);
    let mut out = io::stdout().lock();
    emit(
        &mut out,
        &json!({"ready":true,"oracle":rows.name(),"rows":requests.len(),"base":base.len()}),
    )?;
    for line in io::stdin().lock().lines() {
        let line = line.map_err(|error| error.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let job = Job::parse(&line)?;
        Runner {
            rows,
            requests: &by_id,
            base: &base,
            dump_dir,
        }
        .job(&job, &mut out)?;
    }
    Ok(())
}

/// Runs kill jobs against the base rows, printing the protocol lines.
pub struct Runner<'a, R> {
    pub rows: &'a R,
    pub requests: &'a HashMap<&'a str, &'a Request>,
    pub base: &'a HashMap<String, BaseRow>,
    pub dump_dir: &'a Path,
}

impl<R: Rows> Runner<'_, R> {
    fn dump(&self, name: &str, output: &RowOutput) -> Result<String, String> {
        write_dump(
            self.dump_dir,
            name,
            output.frames.as_deref().unwrap_or_default(),
        )
    }

    /// The control run of a differing row: its result line and whether it
    /// confirms the mutant's difference. `mutant_differs` are the stages where
    /// the mutant differs from the base; the control confirms the kill only
    /// when it completes like the base and the mutant also differs from it in
    /// one of those stages. A control that differs from the mutant only where
    /// the mutant equals the base shows the replacement's side effects, not
    /// its value, moving a digest.
    fn control(
        &self,
        control: MutantId,
        job: &Job,
        request: &Request,
        base_row: &BaseRow,
        (mutant, mutant_differs): (&RowOutput, &[String]),
        out: &mut impl Write,
    ) -> Result<(Value, bool), String> {
        emit(
            out,
            &json!({"heartbeat":request.id,"mutant":job.mutant,"control":control}),
        )?;
        let capture = job.dump_all || self.rows.dumps_kills();
        let executed = run_row(self.rows, request, capture, control);
        let crash = crashed(base_row, &executed.output);
        let differs = differing(
            self.rows.compared(),
            &mutant.digests,
            &executed.output.digests,
        );
        let dump = if capture {
            Some(self.dump(
                &format!("m{}-c{control}-r{}.ndjson", job.mutant, request.index),
                &executed.output,
            )?)
        } else {
            None
        };
        let mut line = row_fields(&executed.output);
        line.insert("id".into(), json!(control));
        line.insert("differs".into(), json!(differs));
        line.insert("crash".into(), json!(crash));
        line.insert("dump".into(), json!(dump));
        let confirms = !crash && differs.iter().any(|stage| mutant_differs.contains(stage));
        Ok((Value::Object(line), confirms))
    }

    pub fn job(&self, job: &Job, out: &mut impl Write) -> Result<(), String> {
        let mutant = job.mutant;
        let (mut ran, mut kills, mut crashes) = (0_usize, 0_usize, 0_usize);
        let mut kill_rows = Vec::new();
        for row in &job.rows {
            let (Some(request), Some(base_row)) =
                (self.requests.get(row.as_str()), self.base.get(row))
            else {
                emit(
                    out,
                    &json!({"mutant":mutant,"row":row,"error":"row has no request or base"}),
                )?;
                continue;
            };
            emit(out, &json!({"heartbeat":row,"mutant":mutant}))?;
            let executed = run_row(self.rows, request, job.dump_all, mutant);
            ran += 1;
            let crash = crashed(base_row, &executed.output);
            let differs = differing(
                self.rows.compared(),
                &executed.output.digests,
                &base_row.digests,
            );
            let mut kill = !crash && !differs.is_empty();
            let mut dump = None;
            let mut nondeterministic = false;
            if job.dump_all {
                dump = Some(self.dump(
                    &format!("m{mutant}-r{}.ndjson", request.index),
                    &executed.output,
                )?);
            } else if kill && self.rows.dumps_kills() {
                // Frames are captured only for differing rows, by running the
                // row again with the same mutant.
                emit(out, &json!({"heartbeat":row,"mutant":mutant,"dump":true}))?;
                let again = run_row(self.rows, request, true, mutant);
                nondeterministic = again.output.digests != executed.output.digests
                    || again.output.outcomes != executed.output.outcomes;
                dump = Some(self.dump(
                    &format!("m{mutant}-r{}.ndjson", request.index),
                    &again.output,
                )?);
            }
            let mut control = None;
            if let (true, Some(id)) = (kill, job.control) {
                let (line, confirms) = self.control(
                    id,
                    job,
                    request,
                    base_row,
                    (&executed.output, &differs),
                    out,
                )?;
                kill = confirms;
                control = Some(line);
            }
            if kill {
                kills += 1;
                kill_rows.push(*request);
            }
            if crash {
                crashes += 1;
            }
            if !differs.is_empty() || crash || job.report_all {
                let mut line = row_fields(&executed.output);
                line.insert("mutant".into(), json!(mutant));
                line.insert("row".into(), json!(row));
                line.insert("differs".into(), json!(differs));
                line.insert("crash".into(), json!(crash));
                line.insert("kill".into(), json!(kill));
                line.insert("dump".into(), json!(dump));
                line.insert("micros".into(), json!(executed.micros));
                if let Some(control) = control {
                    line.insert("control".into(), control);
                }
                if nondeterministic {
                    line.insert("nondeterministic".into(), json!(true));
                }
                emit(out, &Value::Object(line))?;
            }
            if job.max_kills > 0 && kills >= job.max_kills {
                break;
            }
        }
        if job.recheck {
            // A kill must come from the mutant, not from state an earlier row or
            // job left in this process: the same row without a mutant is the base.
            for request in kill_rows {
                emit(
                    out,
                    &json!({"heartbeat":request.id,"mutant":mutant,"recheck":true}),
                )?;
                let executed = run_row(self.rows, request, false, 0);
                let base_row = &self.base[&request.id];
                let base_ok = !crashed(base_row, &executed.output)
                    && differing(
                        self.rows.compared(),
                        &executed.output.digests,
                        &base_row.digests,
                    )
                    .is_empty();
                emit(
                    out,
                    &json!({"mutant":mutant,"recheck":request.id,"base_ok":base_ok}),
                )?;
            }
        }
        emit(
            out,
            &json!({"mutant":mutant,"done":true,"ran":ran,"differing":kills,"crashes":crashes}),
        )
    }
}

/// The `--name value` options of a mutation run, each at most once.
pub fn options(args: &[String], known: &[&str]) -> Result<HashMap<String, String>, String> {
    let mut options = HashMap::new();
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        let name = flag
            .strip_prefix("--")
            .filter(|name| known.contains(name))
            .ok_or_else(|| format!("unknown argument {flag:?}"))?;
        let value = args.next().ok_or_else(|| format!("{flag} needs a value"))?;
        if options.insert(name.to_owned(), value.clone()).is_some() {
            return Err(format!("{flag} given twice"));
        }
    }
    Ok(options)
}

/// Runs `trace` or `kill` for an oracle from its command-line options:
/// `--requests FILE` and either `--out FILE [--frames FILE] [--dump-dir DIR]`
/// (trace) or `--base FILE --dump-dir DIR` (kill).
pub fn run_mode(
    rows: &impl Rows,
    mode: &str,
    options: &HashMap<String, String>,
) -> Result<(), String> {
    let required = |name: &str| {
        options
            .get(name)
            .map(String::as_str)
            .ok_or_else(|| format!("{mode} requires --{name}"))
    };
    let requests = load_requests(rows, Path::new(required("requests")?))?;
    let dump_dir = options.get("dump-dir").map(Path::new);
    match mode {
        "trace" => trace(
            rows,
            &requests,
            Path::new(required("out")?),
            options.get("frames").map(Path::new),
            dump_dir,
        ),
        "kill" => kill(
            rows,
            &requests,
            Path::new(required("base")?),
            dump_dir.ok_or("kill requires --dump-dir")?,
        ),
        _ => Err(format!("unknown mode {mode:?}")),
    }
}
