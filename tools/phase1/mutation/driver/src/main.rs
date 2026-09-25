//! Phase 1 mutation witnesses: runs the E1, binder, facts and table oracles in
//! process with one active mutant at a time.
//!
//! ```text
//! phase1_mutation_driver trace --oracle e1|binder|facts|table --requests FILE --out FILE [--frames FILE] [--dump-dir DIR]
//! phase1_mutation_driver kill  --oracle e1|binder|facts|table --requests FILE --base TRACE --dump-dir DIR   < jobs
//! ```
//!
//! Requests are NDJSON lines holding the example request fields plus
//! `request_sha256`, which is checked against the canonical request bytes. The
//! facts oracle reads the S06 parse requests; the table oracle reads the
//! materialized table inventory (`scripts/phase1_tables.py`). Every row runs on one
//! reserved-stack parser worker, so the parser and binder run inline on the
//! thread whose mutant switch state records reach. The protocol itself (trace
//! lines, kill jobs, controls, rechecks) is `jobs.rs`, shared with the
//! `phase1_syntax` harness's mutation mode.

mod binder;
mod canonical;
mod e1;
mod facts;
mod jobs;
#[cfg(test)]
mod jobs_tests;
mod protocol;
mod table;
#[cfg(test)]
mod verbatim;

use jobs::{panic_message, Request, RowOutput, Rows};
use protocol::{Oracle, Session, Strict};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::process::ExitCode;

/// The rows of one driver oracle.
struct Driver(Oracle);

impl Rows for Driver {
    fn name(&self) -> &'static str {
        self.0.name()
    }

    fn compared(&self) -> &'static [&'static str] {
        self.0.compared()
    }

    fn observe_counts(&self) -> bool {
        self.0.observe_counts()
    }

    fn dumps_kills(&self) -> bool {
        // Binder kills are confirmed on normalized frames; facts kills keep
        // their node lists for review. E1 digests are the comparator's own.
        self.0 != Oracle::E1
    }

    fn request(&self, index: usize, line: &[u8]) -> Result<Request, String> {
        let Strict(mut value) = serde_json::from_slice(line).map_err(|error| error.to_string())?;
        let claimed = value
            .as_object_mut()
            .and_then(|object| object.remove("request_sha256"))
            .and_then(|digest| digest.as_str().map(str::to_owned))
            .ok_or("missing request_sha256")?;
        let mut canonical = Vec::new();
        canonical::write(&mut canonical, &value, false).map_err(|error| error.to_string())?;
        if jobs::sha256_hex(&canonical) != claimed {
            return Err("request_sha256 does not match the request".into());
        }
        if value["op"] != self.0.op() {
            return Err(format!("expected {} requests", self.0.op()));
        }
        match self.0 {
            Oracle::E1 | Oracle::Facts => e1::check(&value),
            Oracle::Binder => binder::check(&value),
            Oracle::Table => table::check(&value),
        }?;
        let id = value["id"].as_str().expect("validated id").to_owned();
        let source_bytes = if self.0 == Oracle::Table {
            table::size(&value)
        } else {
            value["source_hex"].as_str().map_or(0, |hex| hex.len() / 2)
        };
        Ok(Request {
            index,
            id,
            sha256: claimed,
            value,
            source_bytes,
        })
    }

    fn execute(&self, request: &Request, capture: bool) -> RowOutput {
        let session = Session::new(self.0, &request.id, capture);
        let escaped = catch_unwind(AssertUnwindSafe(|| match self.0 {
            Oracle::E1 => e1::run(&session, &request.value),
            Oracle::Binder => binder::run(&session, &request.value),
            Oracle::Facts => facts::run(&session, &request.value),
            Oracle::Table => table::run(&session, &request.value),
        }));
        let mut output = session.finish();
        if let Err(payload) = escaped {
            // The binder example observes outside its stages and would abort here.
            output.error.get_or_insert(format!(
                "observer panic: {}",
                panic_message(payload.as_ref())
            ));
        }
        output
    }
}

fn main() -> ExitCode {
    // Stage panics are recorded as outcomes; the default hook would only
    // repeat them on stderr for every mutant.
    std::panic::set_hook(Box::new(|_| {}));
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = std::thread::scope(|scope| {
        match tsr_parser::spawn_parser_worker(scope, move || run(&args)) {
            Ok(worker) => worker.join().unwrap_or_else(|payload| {
                Err(format!("driver panic: {}", panic_message(payload.as_ref())))
            }),
            Err(error) => Err(format!("could not start the parser worker: {error}")),
        }
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("phase1_mutation_driver: {error}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let (mode, rest) = args
        .split_first()
        .ok_or("usage: phase1_mutation_driver trace|kill --oracle e1|binder|facts|table ...")?;
    let mut options = jobs::options(
        rest,
        &["oracle", "requests", "out", "frames", "dump-dir", "base"],
    )?;
    let oracle = Oracle::parse(
        &options
            .remove("oracle")
            .ok_or_else(|| format!("{mode} requires --oracle"))?,
    )?;
    jobs::run_mode(&Driver(oracle), mode, &options)
}

#[cfg(test)]
mod tests {
    use super::{Driver, Oracle};
    use crate::jobs::{run_row, Request, Rows};
    use serde_json::{json, Value};

    fn request(source: &str) -> Request {
        let value = json!({
            "version": 1, "id": "row", "primary": null, "op": "parse",
            "source_hex": crate::protocol::hex(source.as_bytes()), "filename": "/a.ts", "path": "/a.ts",
            "script_kind": 3, "jsx": false, "force": false,
            "operations": ["parse", "node_index_before", "encode_source_file", "node_index_after"],
        });
        Request {
            index: 0,
            id: "row".into(),
            sha256: String::new(),
            value,
            source_bytes: source.len(),
        }
    }

    #[test]
    fn facts_rows_parse_then_list_every_node_with_its_subtree_facts() {
        let row = run_row(&Driver(Oracle::Facts), &request("let x = a ?? b;"), true, 0);
        let output = row.output;
        assert!(output.error.is_none(), "{:?}", output.error);
        assert_eq!(output.outcomes["parse"], "ok");
        assert_eq!(output.outcomes["subtree_facts"], "ok");
        let frames: Vec<Value> = String::from_utf8(output.frames.unwrap())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let facts = frames
            .iter()
            .find(|frame| frame["tag"] == "observation")
            .unwrap();
        let list = facts["value"].as_array().unwrap();
        // SourceFile first (document order), then its statement, ..., then EOF.
        assert_eq!(
            list[0][0],
            i64::from(tsr_ast::SyntaxKind::SourceFile as i16),
            "the source file comes first"
        );
        assert!(list.len() > 5);
        let nullish = tsr_ast::subtree_flags::NULLISH_COALESCING;
        assert_ne!(
            list[0][1].as_u64().unwrap() & u64::from(nullish),
            0,
            "the file contains nullish coalescing"
        );
        let mut canonical = Vec::new();
        crate::canonical::write(&mut canonical, &facts["value"], false).unwrap();
        assert_eq!(
            output.digests["subtree_facts"],
            crate::jobs::sha256_hex(&canonical),
            "the digest is sha256(canonical(list)) with no newline"
        );
        assert_eq!(output.digests.len(), 1);
        let big = run_row(
            &Driver(Oracle::Facts),
            &request(&"x;".repeat(200_000)),
            true,
            0,
        );
        assert!(
            big.output.error.is_none(),
            "a node list above the example's response limit is one facts frame"
        );
        assert!(big.output.frames.unwrap().len() > crate::protocol::MAX_RESPONSE);
        assert!(Driver(Oracle::Facts).dumps_kills());
        assert!(!Driver(Oracle::Facts).observe_counts());
    }
}
