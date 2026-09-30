//! The checker's trace seam (docs/PHASE2-C6-plan.md, C6.2; ADR 0014).
//!
//! The pin's checker takes an optional `*Tracer` in `NewChecker` and gates every
//! trace call with `if tr := c.tracer; tr != nil`. The tracer forwards events to
//! the program's `tracing.Tracing` session and records every created type with
//! the session's type tracer for that checker. Here the session is a
//! [`TraceSink`] with the shapes of `tracing.Tracing`'s `Push` and `Instant` and
//! of `tracing.Tracer`'s `RecordType`, so the checker does not depend on the
//! Phase 4 tracing package: that package's file writer, `generateTrace` and the
//! thread bookkeeping stay Phase 4's. [`MemoryTraceSink`] keeps the events and
//! recorded types in memory, and [`JsonLinesTraceSink`] writes them as JSON lines
//! without timestamps, process or thread ids.
//!
//! A recorded type is its id. The pin's type tracer keeps the type itself and
//! reads it through `tracedTypeAdapter` when the session stops, after checking;
//! [`crate::Operation::trace_type_records`] reads the checker's types the same
//! way, at the time the caller dumps them.
use crate::{flags, CheckerState, Error, TypeId};
use std::collections::BTreeMap;
use std::io::Write;
use std::sync::{Arc, Mutex};
use tsr_arena::NodeId;

/// `tracing.Phase`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TracePhase {
    Parse,
    Program,
    Bind,
    Check,
    CheckTypes,
    Emit,
    Session,
}

impl TracePhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Parse => "parse",
            Self::Program => "program",
            Self::Bind => "bind",
            Self::Check => "check",
            Self::CheckTypes => "checkTypes",
            Self::Emit => "emit",
            Self::Session => "session",
        }
    }
}

/// A trace argument value: the pin's arguments are integers (node kinds,
/// positions, type ids and counts), strings (file names) and string lists
/// (variances).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TraceValue {
    Int(i64),
    Str(String),
    Strs(Vec<String>),
}

/// The pin's `map[string]any` arguments; Go serializes a map with its keys
/// sorted, which a `BTreeMap` iterates in.
pub type TraceArgs = BTreeMap<String, TraceValue>;

/// The trace session a checker writes to: the shapes of `tracing.Tracing`'s
/// `Push` and `Instant` and of the type tracer's `RecordType` (ADR 0014).
pub trait TraceSink: Send + Sync {
    /// `Tracing.Push`: begins an event and returns the token that ends it.
    /// With `separate_begin_and_end` the session writes a begin event now and
    /// an end event when the token ends; without it, the event is a sampled
    /// one that the pin records only when its duration crosses a sampling
    /// boundary, and never in deterministic mode.
    fn push(
        &self,
        phase: TracePhase,
        name: &str,
        args: &TraceArgs,
        separate_begin_and_end: bool,
    ) -> u64;
    /// The function `Push` returns: ends the event, with its arguments as the
    /// caller holds them at the end.
    fn pop(&self, token: u64, args: &TraceArgs);
    /// `Tracing.Instant`.
    fn instant(&self, phase: TracePhase, name: &str, args: &TraceArgs);
    /// `NewTypeTracer(checkerIndex).RecordType`, by the type's id.
    fn record_type(&self, checker_index: usize, type_id: u32);
}

/// The checker's tracer: a session and the checker's index in its pool.
#[derive(Clone)]
pub struct Tracer {
    sink: Arc<dyn TraceSink>,
    checker_index: usize,
}

impl std::fmt::Debug for Tracer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tracer")
            .field("checker_index", &self.checker_index)
            .finish_non_exhaustive()
    }
}

/// An open event; dropping it ends the event, as the pin's deferred pop does
/// on every return path.
#[must_use = "an unused span ends its event at once"]
pub struct TraceSpan {
    sink: Arc<dyn TraceSink>,
    token: u64,
    checker_index: usize,
    separate_begin_and_end: bool,
    args: TraceArgs,
}

impl TraceSpan {
    /// The arguments the end event carries; `getVariancesWorker` adds the
    /// computed variances before its event ends.
    pub fn args_mut(&mut self) -> &mut TraceArgs {
        &mut self.args
    }
}

impl Drop for TraceSpan {
    fn drop(&mut self) {
        if self.separate_begin_and_end {
            // The pin's end event carries the checker index again
            // (`temporarilyAddCheckerIndex`) over the caller's current arguments.
            let (args, _) = with_checker_index(&self.args, self.checker_index);
            self.sink.pop(self.token, &args);
        } else {
            self.sink.pop(self.token, &self.args);
        }
    }
}

impl Tracer {
    // port: tsc/internal/checker/tracer.go:NewTracer
    pub fn new(sink: Arc<dyn TraceSink>, checker_index: usize) -> Self {
        Self {
            sink,
            checker_index,
        }
    }

    pub fn checker_index(&self) -> usize {
        self.checker_index
    }

    // port: tsc/internal/checker/tracer.go:Tracer.RecordType
    pub(crate) fn record_type(&self, id: TypeId) {
        self.sink.record_type(self.checker_index, id.get());
    }

    // port: tsc/internal/checker/tracer.go:Tracer.Push
    pub(crate) fn push(
        &self,
        phase: TracePhase,
        name: &str,
        args: TraceArgs,
        separate_begin_and_end: bool,
    ) -> TraceSpan {
        let (begin, args) = if separate_begin_and_end {
            // The begin event carries the checker index, and the caller keeps its
            // own arguments without it.
            let (begin, _) = with_checker_index(&args, self.checker_index);
            (begin, args)
        } else {
            let copy = self.copy_with_checker_index(&args);
            (copy.clone(), copy)
        };
        let token = self.sink.push(phase, name, &begin, separate_begin_and_end);
        TraceSpan {
            sink: Arc::clone(&self.sink),
            token,
            checker_index: self.checker_index,
            separate_begin_and_end,
            args,
        }
    }

    // port: tsc/internal/checker/tracer.go:Tracer.Instant
    pub(crate) fn instant(&self, phase: TracePhase, name: &str, args: &TraceArgs) {
        self.sink
            .instant(phase, name, &self.copy_with_checker_index(args));
    }

    // port: tsc/internal/checker/tracer.go:Tracer.copyWithCheckerIndex
    fn copy_with_checker_index(&self, args: &TraceArgs) -> TraceArgs {
        let mut copy = args.clone();
        copy.insert(
            "checkerId".into(),
            TraceValue::Int(index_value(self.checker_index)),
        );
        copy
    }
}

// port: tsc/internal/checker/tracer.go:Tracer.temporarilyAddCheckerIndex
/// The arguments with the checker index added, and whether an earlier
/// `checkerId` was replaced. The pin adds the index to the caller's map and
/// restores it after writing; here the caller's map is never changed.
fn with_checker_index(args: &TraceArgs, checker_index: usize) -> (TraceArgs, bool) {
    let mut added = args.clone();
    let had_previous = added
        .insert(
            "checkerId".into(),
            TraceValue::Int(index_value(checker_index)),
        )
        .is_some();
    (added, had_previous)
}

fn index_value(index: usize) -> i64 {
    i64::try_from(index).unwrap_or(i64::MAX)
}

/// Trace argument helpers for the checker's call sites.
pub(crate) fn int(value: impl TryInto<i64>) -> TraceValue {
    TraceValue::Int(value.try_into().unwrap_or(i64::MAX))
}

pub(crate) fn args<const N: usize>(entries: [(&str, TraceValue); N]) -> TraceArgs {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

/// One event a sink received.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceEvent {
    /// `B` and `E` for an event with separate begin and end, `I` for an
    /// instant, `X` for a sampled event the sink kept.
    pub ph: &'static str,
    pub phase: TracePhase,
    pub name: String,
    pub args: TraceArgs,
}

#[derive(Debug, Default)]
struct MemoryTrace {
    events: Vec<TraceEvent>,
    open: BTreeMap<u64, (TracePhase, String, bool)>,
    next_token: u64,
    types: BTreeMap<usize, Vec<u32>>,
}

/// A session kept in memory. In deterministic mode, which the pin's tests and
/// baselines use, sampled events are never recorded, as the pin skips them;
/// otherwise a sampled event is kept as one `X` event with its start
/// arguments, without its timing.
#[derive(Debug, Default)]
pub struct MemoryTraceSink {
    deterministic: bool,
    state: Mutex<MemoryTrace>,
}

impl MemoryTraceSink {
    pub fn new(deterministic: bool) -> Self {
        Self {
            deterministic,
            state: Mutex::default(),
        }
    }

    pub fn events(&self) -> Vec<TraceEvent> {
        self.lock().events.clone()
    }

    /// The ids of the types each checker recorded, in creation order.
    pub fn recorded_types(&self) -> BTreeMap<usize, Vec<u32>> {
        self.lock().types.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MemoryTrace> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl TraceSink for MemoryTraceSink {
    fn push(
        &self,
        phase: TracePhase,
        name: &str,
        args: &TraceArgs,
        separate_begin_and_end: bool,
    ) -> u64 {
        let mut state = self.lock();
        state.next_token += 1;
        let token = state.next_token;
        if separate_begin_and_end {
            state.events.push(TraceEvent {
                ph: "B",
                phase,
                name: name.to_string(),
                args: args.clone(),
            });
        }
        state
            .open
            .insert(token, (phase, name.to_string(), separate_begin_and_end));
        token
    }

    fn pop(&self, token: u64, args: &TraceArgs) {
        let mut state = self.lock();
        let Some((phase, name, separate)) = state.open.remove(&token) else {
            return;
        };
        if separate {
            state.events.push(TraceEvent {
                ph: "E",
                phase,
                name,
                args: args.clone(),
            });
        } else if !self.deterministic {
            state.events.push(TraceEvent {
                ph: "X",
                phase,
                name,
                args: args.clone(),
            });
        }
    }

    fn instant(&self, phase: TracePhase, name: &str, args: &TraceArgs) {
        self.lock().events.push(TraceEvent {
            ph: "I",
            phase,
            name: name.to_string(),
            args: args.clone(),
        });
    }

    fn record_type(&self, checker_index: usize, type_id: u32) {
        self.lock()
            .types
            .entry(checker_index)
            .or_default()
            .push(type_id);
    }
}

/// A session written as JSON lines: one object per event, with `ph`, `cat`,
/// `name` and `args` as the pin's `trace.json` events have them, and no
/// timestamp, process or thread id. Sampled events are never written, as in
/// the pin's deterministic mode. Recorded types stay in memory for
/// [`crate::Operation::trace_type_records`]; [`write_type_records`] writes them.
pub struct JsonLinesTraceSink<W: Write + Send> {
    events: MemoryTraceSink,
    output: Mutex<W>,
}

impl<W: Write + Send> JsonLinesTraceSink<W> {
    pub fn new(output: W) -> Self {
        Self {
            events: MemoryTraceSink::new(true),
            output: Mutex::new(output),
        }
    }

    pub fn recorded_types(&self) -> BTreeMap<usize, Vec<u32>> {
        self.events.recorded_types()
    }

    pub fn into_inner(self) -> W {
        self.output
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write(&self, event: &TraceEvent) {
        let mut line = String::new();
        event_json(&mut line, event);
        line.push('\n');
        let mut output = self
            .output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // A trace write failure never fails the check, as the pin keeps the
        // first flush error for StopTracing.
        let _ = output.write_all(line.as_bytes());
    }
}

impl<W: Write + Send> TraceSink for JsonLinesTraceSink<W> {
    fn push(
        &self,
        phase: TracePhase,
        name: &str,
        args: &TraceArgs,
        separate_begin_and_end: bool,
    ) -> u64 {
        let token = self.events.push(phase, name, args, separate_begin_and_end);
        if separate_begin_and_end {
            self.write(&TraceEvent {
                ph: "B",
                phase,
                name: name.to_string(),
                args: args.clone(),
            });
        }
        token
    }

    fn pop(&self, token: u64, args: &TraceArgs) {
        let open = self.events.lock().open.get(&token).cloned();
        self.events.pop(token, args);
        if let Some((phase, name, true)) = open {
            self.write(&TraceEvent {
                ph: "E",
                phase,
                name,
                args: args.clone(),
            });
        }
    }

    fn instant(&self, phase: TracePhase, name: &str, args: &TraceArgs) {
        self.write(&TraceEvent {
            ph: "I",
            phase,
            name: name.to_string(),
            args: args.clone(),
        });
    }

    fn record_type(&self, checker_index: usize, type_id: u32) {
        self.events.record_type(checker_index, type_id);
    }
}

fn json_string(out: &mut String, value: &str) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if u32::from(ch) < 0x20 => out.push_str(&format!("\\u{:04x}", u32::from(ch))),
            ch => out.push(ch),
        }
    }
    out.push('"');
}

fn args_json(out: &mut String, args: &TraceArgs) {
    out.push('{');
    for (index, (key, value)) in args.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        json_string(out, key);
        out.push(':');
        match value {
            TraceValue::Int(value) => out.push_str(&value.to_string()),
            TraceValue::Str(value) => json_string(out, value),
            TraceValue::Strs(values) => {
                out.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    json_string(out, value);
                }
                out.push(']');
            }
        }
    }
    out.push('}');
}

fn event_json(out: &mut String, event: &TraceEvent) {
    out.push_str("{\"ph\":");
    json_string(out, event.ph);
    out.push_str(",\"cat\":");
    json_string(out, event.phase.as_str());
    out.push_str(",\"name\":");
    json_string(out, &event.name);
    if !event.args.is_empty() {
        out.push_str(",\"args\":");
        args_json(out, &event.args);
    }
    if event.ph == "I" {
        out.push_str(",\"s\":\"g\"");
    }
    out.push('}');
}

/// A source location of a traced type: the canonical path and 1-based line
/// and UTF-16 character of the node's first token and end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceLocation {
    pub path: String,
    pub start: (i64, i64),
    pub end: (i64, i64),
}

/// A recorded type as the pin's `types.json` describes it (the fields of
/// `tracing.TypeDescriptor`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TraceTypeRecord {
    pub id: u32,
    pub intrinsic_name: Option<String>,
    pub symbol_name: Option<String>,
    pub recursion_id: Option<usize>,
    pub is_tuple: bool,
    pub union_types: Vec<u32>,
    pub intersection_types: Vec<u32>,
    pub alias_type_arguments: Vec<u32>,
    pub keyof_type: Option<u32>,
    pub indexed_access_object_type: Option<u32>,
    pub indexed_access_index_type: Option<u32>,
    pub conditional_check_type: Option<u32>,
    pub conditional_extends_type: Option<u32>,
    /// `-1` for an unresolved branch, as the pin writes it.
    pub conditional_true_type: Option<i64>,
    pub conditional_false_type: Option<i64>,
    pub substitution_base_type: Option<u32>,
    pub constraint_type: Option<u32>,
    pub instantiated_type: Option<u32>,
    pub type_arguments: Vec<u32>,
    pub reference_location: Option<TraceLocation>,
    pub reverse_mapped_source_type: Option<u32>,
    pub reverse_mapped_mapped_type: Option<u32>,
    pub reverse_mapped_constraint_type: Option<u32>,
    pub evolving_array_element_type: Option<u32>,
    pub evolving_array_final_type: Option<u32>,
    pub destructuring_pattern: Option<TraceLocation>,
    pub first_declaration: Option<TraceLocation>,
    pub flags: Vec<&'static str>,
    pub display: Option<String>,
}

/// Writes type records as the pin's `types.json` array, one record per line.
pub fn write_type_records(
    out: &mut impl Write,
    records: &[TraceTypeRecord],
) -> std::io::Result<()> {
    let mut text = String::from("[");
    for (index, record) in records.iter().enumerate() {
        type_record_json(&mut text, record);
        if index + 1 < records.len() {
            text.push_str(",\n");
        }
    }
    text.push_str("]\n");
    out.write_all(text.as_bytes())
}

fn type_record_json(out: &mut String, record: &TraceTypeRecord) {
    fn ids(out: &mut String, key: &str, values: &[u32]) {
        if !values.is_empty() {
            out.push_str(&format!(",\"{key}\":["));
            out.push_str(
                &values
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
            );
            out.push(']');
        }
    }
    fn id(out: &mut String, key: &str, value: Option<impl std::fmt::Display>) {
        if let Some(value) = value {
            out.push_str(&format!(",\"{key}\":{value}"));
        }
    }
    fn location(out: &mut String, key: &str, value: Option<&TraceLocation>) {
        if let Some(value) = value {
            out.push_str(&format!(",\"{key}\":{{\"path\":"));
            json_string(out, &value.path);
            out.push_str(&format!(
                ",\"start\":{{\"line\":{},\"character\":{}}},\"end\":{{\"line\":{},\"character\":{}}}}}",
                value.start.0, value.start.1, value.end.0, value.end.1
            ));
        }
    }
    out.push_str(&format!("{{\"id\":{}", record.id));
    if let Some(name) = &record.intrinsic_name {
        out.push_str(",\"intrinsicName\":");
        json_string(out, name);
    }
    if let Some(name) = &record.symbol_name {
        out.push_str(",\"symbolName\":");
        json_string(out, name);
    }
    id(out, "recursionId", record.recursion_id);
    if record.is_tuple {
        out.push_str(",\"isTuple\":true");
    }
    ids(out, "unionTypes", &record.union_types);
    ids(out, "intersectionTypes", &record.intersection_types);
    ids(out, "aliasTypeArguments", &record.alias_type_arguments);
    id(out, "keyofType", record.keyof_type);
    id(
        out,
        "indexedAccessObjectType",
        record.indexed_access_object_type,
    );
    id(
        out,
        "indexedAccessIndexType",
        record.indexed_access_index_type,
    );
    id(out, "conditionalCheckType", record.conditional_check_type);
    id(
        out,
        "conditionalExtendsType",
        record.conditional_extends_type,
    );
    id(out, "conditionalTrueType", record.conditional_true_type);
    id(out, "conditionalFalseType", record.conditional_false_type);
    id(out, "substitutionBaseType", record.substitution_base_type);
    id(out, "constraintType", record.constraint_type);
    id(out, "instantiatedType", record.instantiated_type);
    ids(out, "typeArguments", &record.type_arguments);
    location(out, "referenceLocation", record.reference_location.as_ref());
    id(
        out,
        "reverseMappedSourceType",
        record.reverse_mapped_source_type,
    );
    id(
        out,
        "reverseMappedMappedType",
        record.reverse_mapped_mapped_type,
    );
    id(
        out,
        "reverseMappedConstraintType",
        record.reverse_mapped_constraint_type,
    );
    id(
        out,
        "evolvingArrayElementType",
        record.evolving_array_element_type,
    );
    id(
        out,
        "evolvingArrayFinalType",
        record.evolving_array_final_type,
    );
    location(
        out,
        "destructuringPattern",
        record.destructuring_pattern.as_ref(),
    );
    location(out, "firstDeclaration", record.first_declaration.as_ref());
    out.push_str(",\"flags\":[");
    for (index, flag) in record.flags.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        json_string(out, flag);
    }
    out.push(']');
    if let Some(display) = &record.display {
        out.push_str(",\"display\":");
        json_string(out, display);
    }
    out.push('}');
}

/// The pin's `tracedTypeAdapter`: a type read through the checker that made it.
pub(crate) struct TracedTypeAdapter<'a> {
    state: &'a mut CheckerState,
    ty: TypeId,
}

// port: tsc/internal/checker/tracer.go:wrapType
pub(crate) fn wrap_type(state: &mut CheckerState, ty: TypeId) -> TracedTypeAdapter<'_> {
    TracedTypeAdapter { state, ty }
}

// port: tsc/internal/checker/tracer.go:wrapTypes
fn wrap_types(types: &[TypeId]) -> Vec<u32> {
    types.iter().map(|ty| ty.get()).collect()
}

impl TracedTypeAdapter<'_> {
    fn record(&self) -> Result<crate::types::TypeRecord, Error> {
        Ok(*self.state.types.get(self.ty)?)
    }

    fn has_object_flag(&self, flag: u32) -> Result<bool, Error> {
        let record = self.record()?;
        Ok(record.flags & flags::type_flags::OBJECT != 0 && record.object_flags & flag != 0)
    }

    // port: tsc/internal/checker/tracer.go:tracedTypeAdapter.Id
    pub(crate) fn id(&self) -> u32 {
        self.ty.get()
    }

    // port: tsc/internal/checker/tracer.go:tracedTypeAdapter.FormatFlags
    fn format_flags(&self) -> Result<Vec<&'static str>, Error> {
        Ok(flags::format_type_flags(self.record()?.flags))
    }

    // port: tsc/internal/checker/tracer.go:tracedTypeAdapter.IsConditional
    fn is_conditional(&self) -> Result<bool, Error> {
        Ok(self.record()?.flags & flags::type_flags::CONDITIONAL != 0)
    }

    // port: tsc/internal/checker/tracer.go:tracedTypeAdapter.Symbol
    fn symbol(&self) -> Result<Option<tsr_arena::SymbolId>, Error> {
        Ok(self.record()?.symbol)
    }

    // port: tsc/internal/checker/tracer.go:tracedTypeAdapter.AliasSymbol
    fn alias_symbol(&self) -> Result<Option<tsr_arena::SymbolId>, Error> {
        Ok(self
            .state
            .types
            .alias_of(self.ty)?
            .map(|alias| alias.symbol))
    }

    // port: tsc/internal/checker/tracer.go:tracedTypeAdapter.AliasTypeArguments
    fn alias_type_arguments(&self) -> Result<Vec<u32>, Error> {
        Ok(match self.state.types.alias_of(self.ty)? {
            Some(alias) => wrap_types(&alias.type_arguments),
            None => Vec::new(),
        })
    }

    // port: tsc/internal/checker/tracer.go:tracedTypeAdapter.IntrinsicName
    fn intrinsic_name(&self) -> Result<Option<String>, Error> {
        let record = self.record()?;
        if record.flags & flags::type_flags::INTRINSIC == 0
            || record.kind() != crate::types::TypeKind::Intrinsic
        {
            return Ok(None);
        }
        Ok(Some(
            String::from_utf8_lossy(self.state.types.intrinsic(self.ty)?.name.as_bytes())
                .into_owned(),
        ))
    }

    // port: tsc/internal/checker/tracer.go:tracedTypeAdapter.UnionTypes
    fn union_types(&self) -> Result<Vec<u32>, Error> {
        if self.record()?.flags & flags::type_flags::UNION == 0 {
            return Ok(Vec::new());
        }
        Ok(wrap_types(&self.state.types.union(self.ty)?.types))
    }

    // port: tsc/internal/checker/tracer.go:tracedTypeAdapter.IntersectionTypes
    fn intersection_types(&self) -> Result<Vec<u32>, Error> {
        if self.record()?.flags & flags::type_flags::INTERSECTION == 0 {
            return Ok(Vec::new());
        }
        Ok(wrap_types(&self.state.types.intersection(self.ty)?.types))
    }

    // port: tsc/internal/checker/tracer.go:tracedTypeAdapter.IndexType
    fn index_type(&self) -> Result<Option<u32>, Error> {
        if self.record()?.flags & flags::type_flags::INDEX == 0 {
            return Ok(None);
        }
        Ok(Some(self.state.types.index_type(self.ty)?.target.get()))
    }

    // port: tsc/internal/checker/tracer.go:tracedTypeAdapter.IndexedAccessObjectType
    fn indexed_access_object_type(&self) -> Result<Option<u32>, Error> {
        if self.record()?.flags & flags::type_flags::INDEXED_ACCESS == 0 {
            return Ok(None);
        }
        Ok(Some(
            self.state.types.indexed_access(self.ty)?.object_type.get(),
        ))
    }

    // port: tsc/internal/checker/tracer.go:tracedTypeAdapter.IndexedAccessIndexType
    fn indexed_access_index_type(&self) -> Result<Option<u32>, Error> {
        if self.record()?.flags & flags::type_flags::INDEXED_ACCESS == 0 {
            return Ok(None);
        }
        Ok(Some(
            self.state.types.indexed_access(self.ty)?.index_type.get(),
        ))
    }

    fn conditional(&self) -> Result<Option<&crate::types::ConditionalData>, Error> {
        if self.record()?.flags & flags::type_flags::CONDITIONAL == 0 {
            return Ok(None);
        }
        self.state.types.conditional(self.ty).map(Some)
    }

    // port: tsc/internal/checker/tracer.go:tracedTypeAdapter.ConditionalCheckType
    fn conditional_check_type(&self) -> Result<Option<u32>, Error> {
        Ok(self.conditional()?.map(|data| data.check_type.get()))
    }

    // port: tsc/internal/checker/tracer.go:tracedTypeAdapter.ConditionalExtendsType
    fn conditional_extends_type(&self) -> Result<Option<u32>, Error> {
        Ok(self.conditional()?.map(|data| data.extends_type.get()))
    }

    // port: tsc/internal/checker/tracer.go:tracedTypeAdapter.ConditionalTrueType
    fn conditional_true_type(&self) -> Result<Option<u32>, Error> {
        Ok(self
            .conditional()?
            .and_then(|data| data.true_type)
            .map(TypeId::get))
    }

    // port: tsc/internal/checker/tracer.go:tracedTypeAdapter.ConditionalFalseType
    fn conditional_false_type(&self) -> Result<Option<u32>, Error> {
        Ok(self
            .conditional()?
            .and_then(|data| data.false_type)
            .map(TypeId::get))
    }

    // port: tsc/internal/checker/tracer.go:tracedTypeAdapter.SubstitutionBaseType
    fn substitution_base_type(&self) -> Result<Option<u32>, Error> {
        if self.record()?.flags & flags::type_flags::SUBSTITUTION == 0 {
            return Ok(None);
        }
        Ok(Some(self.state.types.substitution(self.ty)?.base.get()))
    }

    // port: tsc/internal/checker/tracer.go:tracedTypeAdapter.SubstitutionConstraintType
    fn substitution_constraint_type(&self) -> Result<Option<u32>, Error> {
        if self.record()?.flags & flags::type_flags::SUBSTITUTION == 0 {
            return Ok(None);
        }
        Ok(Some(
            self.state.types.substitution(self.ty)?.constraint.get(),
        ))
    }

    fn reference(&self) -> Result<Option<&crate::types::ReferenceData>, Error> {
        if !self.has_object_flag(flags::object_flags::REFERENCE)? {
            return Ok(None);
        }
        self.state.types.type_reference(self.ty).map(Some)
    }

    // port: tsc/internal/checker/tracer.go:tracedTypeAdapter.ReferenceTarget
    fn reference_target(&self) -> Result<Option<u32>, Error> {
        Ok(self
            .reference()?
            .and_then(|data| data.object.target)
            .map(TypeId::get))
    }

    // port: tsc/internal/checker/tracer.go:tracedTypeAdapter.ReferenceTypeArguments
    fn reference_type_arguments(&self) -> Result<Vec<u32>, Error> {
        Ok(self
            .reference()?
            .and_then(|data| data.resolved_type_arguments.as_ref())
            .map(|list| wrap_types(list))
            .unwrap_or_default())
    }

    // port: tsc/internal/checker/tracer.go:tracedTypeAdapter.ReferenceNode
    fn reference_node(&self) -> Result<Option<NodeId>, Error> {
        Ok(self.reference()?.and_then(|data| data.node))
    }

    fn reverse_mapped(&self) -> Result<Option<&crate::types::ReverseMappedData>, Error> {
        if !self.has_object_flag(flags::object_flags::REVERSE_MAPPED)? {
            return Ok(None);
        }
        self.state.types.reverse_mapped(self.ty).map(Some)
    }

    // port: tsc/internal/checker/tracer.go:tracedTypeAdapter.ReverseMappedSourceType
    fn reverse_mapped_source_type(&self) -> Result<Option<u32>, Error> {
        Ok(self
            .reverse_mapped()?
            .and_then(|data| data.source)
            .map(TypeId::get))
    }

    // port: tsc/internal/checker/tracer.go:tracedTypeAdapter.ReverseMappedMappedType
    fn reverse_mapped_mapped_type(&self) -> Result<Option<u32>, Error> {
        Ok(self
            .reverse_mapped()?
            .and_then(|data| data.mapped_type)
            .map(TypeId::get))
    }

    // port: tsc/internal/checker/tracer.go:tracedTypeAdapter.ReverseMappedConstraintType
    fn reverse_mapped_constraint_type(&self) -> Result<Option<u32>, Error> {
        Ok(self
            .reverse_mapped()?
            .and_then(|data| data.constraint_type)
            .map(TypeId::get))
    }

    fn evolving_array(&self) -> Result<Option<&crate::types::EvolvingArrayData>, Error> {
        if !self.has_object_flag(flags::object_flags::EVOLVING_ARRAY)? {
            return Ok(None);
        }
        self.state.types.evolving_array(self.ty).map(Some)
    }

    // port: tsc/internal/checker/tracer.go:tracedTypeAdapter.EvolvingArrayElementType
    fn evolving_array_element_type(&self) -> Result<Option<u32>, Error> {
        Ok(self.evolving_array()?.map(|data| data.element_type.get()))
    }

    // port: tsc/internal/checker/tracer.go:tracedTypeAdapter.EvolvingArrayFinalType
    fn evolving_array_final_type(&self) -> Result<Option<u32>, Error> {
        Ok(self
            .evolving_array()?
            .and_then(|data| data.final_array_type)
            .map(TypeId::get))
    }

    // port: tsc/internal/checker/tracer.go:tracedTypeAdapter.IsTuple
    fn is_tuple(&self) -> Result<bool, Error> {
        Ok(self.record()?.object_flags & flags::object_flags::TUPLE != 0)
    }

    // port: tsc/internal/checker/tracer.go:tracedTypeAdapter.Pattern
    fn pattern(&self) -> Option<NodeId> {
        self.state.bindings.pattern_for_type.get(&self.ty).copied()
    }

    // port: tsc/internal/checker/tracer.go:tracedTypeAdapter.RecursionIdentity
    fn recursion_identity(&mut self) -> Result<crate::constraints::RecursionIdentity, Error> {
        self.state.recursion_identity(self.ty)
    }

    // port: tsc/internal/checker/tracer.go:tracedTypeAdapter.Display
    /// The display text of an anonymous, literal, template literal, union or
    /// intersection type; a failure while displaying an incomplete type yields
    /// no text, as the pin recovers from its panic.
    fn display(&mut self) -> Result<Option<String>, Error> {
        let record = self.record()?;
        let shown = record.object_flags & flags::object_flags::ANONYMOUS != 0
            || record.flags
                & (flags::type_flags::LITERAL
                    | flags::type_flags::TEMPLATE_LITERAL
                    | flags::type_flags::UNION
                    | flags::type_flags::INTERSECTION)
                != 0;
        if !shown {
            return Ok(None);
        }
        let text = self
            .state
            .type_to_string(self.ty, crate::type_display::DEFAULT_FLAGS)
            .map(|text| String::from_utf8_lossy(text.as_bytes()).into_owned())
            .unwrap_or_default();
        Ok((!text.is_empty()).then_some(text))
    }
}

impl CheckerState {
    fn trace_location(&mut self, node: NodeId) -> Result<Option<TraceLocation>, Error> {
        let view = self.ast(node)?;
        let Some(file) = tsr_ast::utilities::get_source_file_of_node(view, Some(node))? else {
            return Ok(None);
        };
        let source = view.source_file(file)?;
        let text = source.text().as_bytes();
        let mut jsdoc = tsr_ast::EagerJsDocProvider::default();
        let start = tsr_scanner::get_token_pos_of_node(view, file, node, false, &mut jsdoc)?;
        let end = i64::from(view.node(node)?.end());
        let position = |at: i64| {
            let (line, character) =
                tsr_jsstring::scanner_positions::get_ecma_line_and_utf16_character_of_position(
                    text,
                    isize::try_from(at).unwrap_or(0),
                );
            (line as i64 + 1, character as i64 + 1)
        };
        let path =
            String::from_utf8_lossy(tsr_tspath::to_path(source.file_name(), b"", false).as_bytes())
                .into_owned();
        Ok(Some(TraceLocation {
            path,
            start: position(start),
            end: position(end),
        }))
    }

    /// The pin's `DumpTypes` over the recorded types of one checker: each
    /// type's record, with recursion identities numbered in first-seen order.
    pub(crate) fn trace_type_records(
        &mut self,
        ids: &[u32],
    ) -> Result<Vec<TraceTypeRecord>, Error> {
        let mut identities: Vec<crate::constraints::RecursionIdentity> = Vec::new();
        let mut records = Vec::with_capacity(ids.len());
        for &raw in ids {
            let ty = TypeId::new(raw).ok_or(Error::MissingLink("recorded trace type id"))?;
            let mut adapter = wrap_type(self, ty);
            let symbol = adapter.symbol()?;
            let alias_symbol = adapter.alias_symbol()?;
            let identity = adapter.recursion_identity()?;
            let recursion_id =
                if let Some(index) = identities.iter().position(|known| *known == identity) {
                    index
                } else {
                    identities.push(identity);
                    identities.len() - 1
                };
            let conditional = adapter.is_conditional()?;
            let mut record = TraceTypeRecord {
                id: adapter.id(),
                flags: adapter.format_flags()?,
                recursion_id: Some(recursion_id),
                intrinsic_name: adapter.intrinsic_name()?,
                is_tuple: adapter.is_tuple()?,
                union_types: adapter.union_types()?,
                intersection_types: adapter.intersection_types()?,
                alias_type_arguments: adapter.alias_type_arguments()?,
                keyof_type: adapter.index_type()?,
                indexed_access_object_type: adapter.indexed_access_object_type()?,
                indexed_access_index_type: adapter.indexed_access_index_type()?,
                substitution_base_type: adapter.substitution_base_type()?,
                constraint_type: adapter.substitution_constraint_type()?,
                instantiated_type: adapter.reference_target()?,
                type_arguments: adapter.reference_type_arguments()?,
                reverse_mapped_source_type: adapter.reverse_mapped_source_type()?,
                reverse_mapped_mapped_type: adapter.reverse_mapped_mapped_type()?,
                reverse_mapped_constraint_type: adapter.reverse_mapped_constraint_type()?,
                evolving_array_element_type: adapter.evolving_array_element_type()?,
                evolving_array_final_type: adapter.evolving_array_final_type()?,
                ..TraceTypeRecord::default()
            };
            if conditional {
                record.conditional_check_type = adapter.conditional_check_type()?;
                record.conditional_extends_type = adapter.conditional_extends_type()?;
                record.conditional_true_type =
                    Some(adapter.conditional_true_type()?.map_or(-1, i64::from));
                record.conditional_false_type =
                    Some(adapter.conditional_false_type()?.map_or(-1, i64::from));
            }
            let reference_node = adapter.reference_node()?;
            let pattern = adapter.pattern();
            record.display = adapter.display()?;
            // The alias symbol names the type when it has one (`aliasSymbol ?? symbol`).
            let named = alias_symbol.or(symbol);
            if let Some(named) = named {
                let name = self.symbol(named)?.name_bytes().to_vec();
                record.symbol_name = Some(
                    String::from_utf8_lossy(&tsr_ast::escape_all_internal_symbol_names(&name))
                        .into_owned(),
                );
            }
            if let Some(node) = reference_node {
                record.reference_location = self.trace_location(node)?;
            }
            if let Some(node) = pattern {
                record.destructuring_pattern = self.trace_location(node)?;
            }
            if let Some(named) = named {
                if let Some(Some(first)) = self.symbol_declarations(named)?.first() {
                    record.first_declaration = self.trace_location(first)?;
                }
            }
            records.push(record);
        }
        Ok(records)
    }
}

impl crate::Operation<'_> {
    /// The records of the given recorded types of this checker, read as the
    /// pin's `DumpTypes` reads them when the session stops.
    pub fn trace_type_records(&mut self, ids: &[u32]) -> Result<Vec<TraceTypeRecord>, Error> {
        self.state_mut().trace_type_records(ids)
    }
}

impl CheckerState {
    /// The `{kind, pos, end, path}` arguments of the pin's node check events.
    pub(crate) fn trace_node_args(&self, node: NodeId) -> Result<TraceArgs, Error> {
        let read = self.node(node)?;
        let view = self.ast(node)?;
        let path = match tsr_ast::utilities::get_source_file_of_node(view, Some(node))? {
            Some(file) => String::from_utf8_lossy(view.source_file(file)?.file_name()).into_owned(),
            None => String::new(),
        };
        Ok(args([
            ("kind", int(read.kind().raw())),
            ("pos", int(read.pos())),
            ("end", int(read.end())),
            ("path", TraceValue::Str(path)),
        ]))
    }

    /// `if tr := c.tracer; tr != nil { defer tr.Push(...)() }`.
    pub(crate) fn trace_span(
        &self,
        phase: TracePhase,
        name: &str,
        args: impl FnOnce(&Self) -> Result<TraceArgs, Error>,
        separate_begin_and_end: bool,
    ) -> Result<Option<TraceSpan>, Error> {
        let Some(tracer) = &self.tracer else {
            return Ok(None);
        };
        let args = args(self)?;
        Ok(Some(tracer.push(phase, name, args, separate_begin_and_end)))
    }

    /// A sampled check event over a node, as `checkExpression`,
    /// `checkVariableDeclaration` and `checkDeferredNode` push it.
    pub(crate) fn trace_node_span(
        &self,
        name: &str,
        node: NodeId,
    ) -> Result<Option<TraceSpan>, Error> {
        self.trace_span(
            TracePhase::Check,
            name,
            |state| state.trace_node_args(node),
            false,
        )
    }

    /// `if tr := c.tracer; tr != nil { tr.Instant(...) }`.
    pub(crate) fn trace_instant(
        &self,
        phase: TracePhase,
        name: &str,
        args: impl FnOnce() -> TraceArgs,
    ) {
        if let Some(tracer) = &self.tracer {
            tracer.instant(phase, name, &args());
        }
    }
}
