//! The observers' `Session`, digesting per stage instead of printing.
//!
//! The E1, binder and facts observers call `observe`, `stage` and `failure`
//! exactly as the example session of `crates/tsr_encoder/examples/support/
//! protocol.rs` expects. This session keeps the same frame sequence (and can
//! capture the frame bytes the example would print), digests each stage the way
//! the Python evidence comparators do, and switches the mutant stage through
//! `jobs::enter`: `parse`, `bind`, `repeat_bind` and `subtree_facts` run as
//! production, everything else observes.

use crate::canonical;
use crate::jobs::{enter, lower_hex, panic_message, RowOutput, Stage};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};

// The example protocol supplies the shared helpers; its printing session and
// request loop are replaced here.
#[allow(dead_code)]
#[path = "../../../../../crates/tsr_encoder/examples/support/protocol.rs"]
mod example;
pub use example::{fields, hex, unhex, Strict, MAX_RESPONSE};

/// The oracle a driver process serves.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Oracle {
    E1,
    Binder,
    /// Per-node subtree facts after a parse of the S06 primary requests.
    Facts,
}

impl Oracle {
    pub fn parse(name: &str) -> Result<Self, String> {
        match name {
            "e1" => Ok(Self::E1),
            "binder" => Ok(Self::Binder),
            "facts" => Ok(Self::Facts),
            _ => Err(format!("unknown oracle {name:?}")),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::E1 => "e1",
            Self::Binder => "binder",
            Self::Facts => "facts",
        }
    }

    /// The request `op` the oracle serves (facts reuses the S06 parse requests).
    pub fn op(self) -> &'static str {
        match self {
            Self::E1 | Self::Facts => "parse",
            Self::Binder => "bind",
        }
    }

    /// Every stage of a request, in protocol order.
    pub fn operations(self) -> &'static [&'static str] {
        match self {
            Self::E1 => &[
                "parse",
                "node_index_before",
                "encode_source_file",
                "node_index_after",
            ],
            Self::Binder => &[
                "parse",
                "parsed_graph",
                "bind",
                "bound_graph",
                "repeat_bind",
                "repeated_graph",
            ],
            Self::Facts => &["parse", "subtree_facts"],
        }
    }

    /// The stages whose observations are digested and compared.
    pub fn compared(self) -> &'static [&'static str] {
        match self {
            Self::E1 => &[
                "parse",
                "node_index_before",
                "encode_source_file",
                "node_index_after",
            ],
            Self::Binder => &["parsed_graph", "bound_graph", "repeated_graph"],
            Self::Facts => &["subtree_facts"],
        }
    }

    /// Whether observation stages run production parser work whose reach
    /// counts (E1's lazy JSDoc during encoding); elsewhere observing counts
    /// nothing and mutants are off.
    pub fn observe_counts(self) -> bool {
        self == Self::E1
    }
}

/// Stages that run production code; mutants of every crate are live there.
pub fn is_production(stage: &str) -> bool {
    matches!(stage, "parse" | "bind" | "repeat_bind" | "subtree_facts")
}

struct Fragment {
    kind: String,
    parts: i64,
    next: i64,
    bytes: Vec<u8>,
}

struct State {
    oracle: Oracle,
    id: String,
    seq: usize,
    stages: usize,
    frames: Option<Vec<u8>>,
    frame_error: Option<String>,
    digests: BTreeMap<String, Sha256>,
    outcomes: Vec<(String, String, String)>,
    fragment: Option<Fragment>,
    scratch: Vec<u8>,
    error: Option<String>,
}

pub struct Session(RefCell<State>);

impl Session {
    pub fn new(oracle: Oracle, id: &str, capture: bool) -> Self {
        let session = Self(RefCell::new(State {
            oracle,
            id: id.into(),
            seq: 0,
            stages: 0,
            frames: capture.then(Vec::new),
            frame_error: None,
            digests: oracle
                .compared()
                .iter()
                .map(|stage| ((*stage).to_owned(), Sha256::new()))
                .collect(),
            outcomes: Vec::new(),
            fragment: None,
            scratch: Vec::new(),
            error: None,
        }));
        session
            .0
            .borrow_mut()
            .frame(json!({"tag":"begin","op":oracle.op()}));
        session
    }

    pub fn observe(&self, stage: &str, kind: &str, value: Value) {
        enter(Stage::Observe);
        let mut state = self.0.borrow_mut();
        state.digest(stage, kind, &value);
        let seq = state.seq;
        let mut frame = json!({"tag":"observation","seq":seq,"stage":stage,"kind":kind});
        frame["value"] = value;
        state.frame(frame);
        state.seq += 1;
    }

    pub fn stage(&self, stage: &str, action: impl FnOnce() -> Result<(), String>) -> bool {
        enter(if is_production(stage) {
            Stage::Production
        } else {
            Stage::Observe
        });
        let result = catch_unwind(AssertUnwindSafe(action));
        enter(Stage::Observe);
        let (outcome, message) = match result {
            Ok(Ok(())) => ("ok", String::new()),
            Ok(Err(error)) => ("error", error),
            Err(error) => ("panic", panic_message(error.as_ref())),
        };
        let mut state = self.0.borrow_mut();
        state.frame(json!({"tag":"stage","stage":stage,"outcome":outcome,"message_hex":hex(message.as_bytes())}));
        state.stages += 1;
        state
            .outcomes
            .push((stage.to_owned(), outcome.to_owned(), message));
        outcome == "ok"
    }

    pub fn failure(&self, error: String) {
        let mut state = self.0.borrow_mut();
        state.error.get_or_insert(error.clone());
        state.frame_error.get_or_insert(error);
    }

    /// Closes the row: the example's `end` frame, then the digests.
    pub fn finish(self) -> RowOutput {
        let mut state = self.0.into_inner();
        let (seq, stages) = (state.seq, state.stages);
        state.frame(json!({"tag":"end","observations":seq,"stages":stages}));
        if state.fragment.is_some() {
            state
                .error
                .get_or_insert("incomplete graph fragment".into());
        }
        let ran: BTreeMap<String, (String, String)> = state
            .outcomes
            .into_iter()
            .map(|(stage, outcome, message)| (stage, (outcome, message)))
            .collect();
        let outcomes = state
            .oracle
            .operations()
            .iter()
            .map(|stage| {
                let outcome = ran.get(*stage).map_or("not_run", |(outcome, _)| outcome);
                ((*stage).to_owned(), outcome.to_owned())
            })
            .collect();
        let messages = ran
            .iter()
            .filter(|(_, (outcome, _))| outcome != "ok")
            .map(|(stage, (_, message))| (stage.clone(), hex(message.as_bytes())))
            .collect();
        if ran.len() != state.stages
            || ran
                .keys()
                .any(|stage| !state.oracle.operations().contains(&stage.as_str()))
        {
            state
                .error
                .get_or_insert("repeated or unknown stage".into());
        }
        RowOutput {
            outcomes,
            messages,
            digests: state
                .digests
                .into_iter()
                .filter(|(stage, _)| ran.contains_key(stage))
                .map(|(stage, digest)| (stage, lower_hex(&digest.finalize())))
                .collect(),
            frames: state.frames,
            error: state.error,
        }
    }
}

impl State {
    /// The example's frame bytes: identity and version appended last, one
    /// record per line, nothing written after the first failure. The example
    /// protocol's response limit applies to the oracles an example produces
    /// (E1, binder); a facts row's single node list has no such producer.
    fn frame(&mut self, mut value: Value) {
        if self.frame_error.is_some() {
            return;
        }
        let Some(frames) = self.frames.as_mut() else {
            return;
        };
        value["id"] = self.id.clone().into();
        value["version"] = 1.into();
        match serde_json::to_vec(&value) {
            Ok(bytes) if bytes.len() <= MAX_RESPONSE || self.oracle == Oracle::Facts => {
                frames.extend_from_slice(&bytes);
                frames.push(b'\n');
            }
            Ok(_) => self.fail("oversized response record".into()),
            Err(error) => self.fail(error.to_string()),
        }
    }

    fn fail(&mut self, error: String) {
        self.error.get_or_insert(error.clone());
        self.frame_error.get_or_insert(error);
    }

    /// One logical observation, digested by the native file's `digest_rule`:
    /// E1 `canonical({"kind","value"}) + "\n"`; binder, per graph record with
    /// fragments reassembled, `canonical([kind, comparable(value)]) + "\n"`;
    /// facts, the stage's single observation `canonical(value)` (the list of
    /// `[kind, subtree facts]` in document order), with no newline.
    fn digest(&mut self, stage: &str, kind: &str, value: &Value) {
        if self.oracle == Oracle::Binder && kind == "fragment" {
            match self.fragment_part(value) {
                Ok(Some((kind, record))) => self.digest_record(stage, &kind, &record),
                Ok(None) => {}
                Err(error) => {
                    self.error.get_or_insert(error);
                }
            }
            return;
        }
        if self.fragment.is_some() {
            self.error
                .get_or_insert("observation interleaved with a graph fragment".into());
        }
        self.digest_record(stage, kind, value);
    }

    fn digest_record(&mut self, stage: &str, kind: &str, value: &Value) {
        self.scratch.clear();
        let written = match self.oracle {
            Oracle::E1 => canonical::write_observation(&mut self.scratch, kind, value),
            Oracle::Binder => canonical::write_graph_record(&mut self.scratch, kind, value),
            Oracle::Facts => canonical::write(&mut self.scratch, value, false),
        };
        if let Err(error) = written {
            self.error.get_or_insert(error.to_string());
            return;
        }
        if self.oracle != Oracle::Facts {
            self.scratch.push(b'\n');
        }
        self.digests
            .entry(stage.to_owned())
            .or_default()
            .update(&self.scratch);
    }

    fn fragment_part(&mut self, value: &Value) -> Result<Option<(String, Value)>, String> {
        let kind = value["record_kind"].as_str().ok_or("fragment kind")?;
        let part = value["part"].as_i64().ok_or("fragment part")?;
        let parts = value["parts"].as_i64().ok_or("fragment count")?;
        let bytes = unhex(&value["payload_hex"])?;
        let fragment = self.fragment.get_or_insert_with(|| Fragment {
            kind: kind.to_owned(),
            parts,
            next: 0,
            bytes: Vec::new(),
        });
        if fragment.kind != kind || fragment.parts != parts || fragment.next != part {
            return Err("missing, reordered or interleaved graph fragment".into());
        }
        fragment.bytes.extend_from_slice(&bytes);
        fragment.next += 1;
        if fragment.next < fragment.parts {
            return Ok(None);
        }
        let fragment = self.fragment.take().expect("fragment in progress");
        let Strict(record) =
            serde_json::from_slice(&fragment.bytes).map_err(|error| error.to_string())?;
        Ok(Some((fragment.kind, record)))
    }
}

#[cfg(test)]
mod tests {
    use super::{Oracle, Session};
    use phase1_mutants::{stage, Stage};
    use serde_json::json;

    #[test]
    fn stages_switch_the_mutant_stage_and_rows_take_the_native_shape() {
        let session = Session::new(Oracle::Binder, "row", true);
        assert!(session.stage("parse", || {
            assert_eq!(stage(), Stage::Production);
            Ok(())
        }));
        assert_eq!(
            stage(),
            Stage::Observe,
            "observers run after a production stage"
        );
        session.observe("parsed_graph", "source", json!({"a": 1}));
        assert!(session.stage("parsed_graph", || {
            assert_eq!(stage(), Stage::Observe);
            Ok(())
        }));
        assert!(!session.stage("bind", || panic!("boom")));
        let row = session.finish();
        assert_eq!(row.outcomes["parse"], "ok");
        assert_eq!(row.outcomes["bind"], "panic");
        assert_eq!(row.outcomes["repeated_graph"], "not_run");
        assert_eq!(row.messages["bind"], "626f6f6d");
        assert_eq!(
            row.digests.keys().collect::<Vec<_>>(),
            ["parsed_graph"],
            "only compared stages that ran carry a digest"
        );
        assert!(row.error.is_none());
        // Key order follows serde_json's map features, as in the examples.
        let frames: Vec<serde_json::Value> = String::from_utf8(row.frames.unwrap())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            frames.first(),
            Some(&json!({"id":"row","op":"bind","tag":"begin","version":1}))
        );
        assert_eq!(
            frames.last(),
            Some(&json!({"id":"row","observations":1,"stages":3,"tag":"end","version":1}))
        );
        assert_eq!(frames.len(), 6);
        phase1_mutants::set_stage(Stage::Production);
    }

    #[test]
    fn reassembled_graph_fragments_digest_like_the_whole_record() {
        let record = json!({"name": {"raw_hex": "fe31", "identity": {"kind": "node", "ref": 2}}, "items": [1, 2, 3]});
        let whole = Session::new(Oracle::Binder, "row", false);
        whole.observe("bound_graph", "symbol", record.clone());
        assert!(whole.stage("bound_graph", || Ok(())));
        let bytes = serde_json::to_vec(&record).unwrap();
        let split = Session::new(Oracle::Binder, "row", false);
        for (part, chunk) in bytes.chunks(bytes.len().div_ceil(2)).enumerate() {
            split.observe(
                "bound_graph",
                "fragment",
                json!({"record_kind": "symbol", "part": part, "parts": 2, "payload_hex": super::hex(chunk)}),
            );
        }
        assert!(split.stage("bound_graph", || Ok(())));
        let (whole, split) = (whole.finish(), split.finish());
        assert!(split.error.is_none());
        assert_eq!(whole.digests, split.digests);
        let renamed = Session::new(Oracle::Binder, "row", false);
        renamed.observe(
            "bound_graph",
            "symbol",
            json!({"name": {"raw_hex": "fe32", "identity": {"kind": "node", "ref": 2}}, "items": [1, 2, 3]}),
        );
        assert!(renamed.stage("bound_graph", || Ok(())));
        assert_eq!(
            renamed.finish().digests,
            whole.digests,
            "identity-derived name bytes are normalized away"
        );
    }
}
