//! The `tsc` suite's side of the parity contract (docs/EVIDENCE-plan.md,
//! section 4; `scripts/parity.py`): one scenario's row and transcript
//! ([`crate::row::run_scenario`]) as the result lines of the pin's
//! `tscInput.run` subtest (`runner.go`), which checks two things:
//!
//! - `transcript`: `baseline.Run` compares the transcript with the committed
//!   reference `baselines/reference/<id>` byte for byte and writes a
//!   differing one under the local directory;
//! - `incremental`: `t.Errorf` reports what each edit's incremental build
//!   left different from a clean build of the same state, unless the edit
//!   explains it (`expectedDiff`).
//!
//! The pin makes both checks after the run; a panic in the run fails the
//! subtest before either. A run that did not complete (a refusal, a typed
//! error, a panic or a harness defect) therefore fails both sub-tests with
//! the row's reason, and nothing is compared or written.
use serde_json::Value;
use tsr_testrunner::baseline::{self, Options, Roots};
use tsr_testrunner::result::{Outcome, Report};

/// The sub-test comparing the transcript with the committed reference.
pub const TRANSCRIPT: &str = "transcript";
/// The sub-test reporting an unexpected incremental difference.
pub const INCREMENTAL: &str = "incremental";

/// The result lines of a scenario's `row` and `transcript`, transcript
/// first, as the pin checks them. A completed run's transcript is compared
/// with `roots.reference/<id>` and written to `roots.local/<id>` when it
/// differs. The checks are the end of `runner.go:tscInput.run`, whose run
/// itself is [`crate::runner::TscInput::run`].
pub fn report(row: &Value, transcript: &[u8], roots: &Roots) -> Report {
    let id = row["id"].as_str().expect("a row names its scenario");
    let mut report = Report::default();
    if row["state"] == "completed" {
        let compared = baseline::run(roots, id.as_bytes(), transcript, Options::default());
        report.subtest(id, TRANSCRIPT, compared);
        let incremental = match row["unexpected_diff"].as_str() {
            None => Outcome::Pass,
            Some(diff) => Outcome::fail_with(
                "unexpected diff with incremental build, please review the baseline file",
                diff,
            ),
        };
        report.subtest(id, INCREMENTAL, incremental);
        return report;
    }
    let reason = stop_reason(row);
    let detail = stop_detail(row);
    for subtest in [TRANSCRIPT, INCREMENTAL] {
        report.subtest(
            id,
            subtest,
            Outcome::fail_with(reason.clone(), detail.clone()),
        );
    }
    report
}

/// Why a run stopped: the refused operation, or the failure's class and
/// reason.
fn stop_reason(row: &Value) -> String {
    let text = |key: &str| row[key].as_str().unwrap_or("").to_owned();
    match row["state"].as_str() {
        Some("unsupported") => format!("unsupported: {}", text("operation")),
        _ => format!("{}: {}", text("class"), text("reason")),
    }
}

/// Where a run stopped: how far its transcript got and, for a panic, where
/// the hook saw it.
fn stop_detail(row: &Value) -> String {
    let progress = &row["progress"];
    let mut detail = format!(
        "stopped in stage {} after {} command(s), {} of {} edit(s) completed",
        progress["stage"].as_str().unwrap_or(""),
        progress["commands"],
        progress["edits_completed"],
        progress["edits"],
    );
    if let Some(location) = row["location"].as_str() {
        detail.push_str(&format!("; at {location}"));
    }
    detail
}

#[cfg(test)]
mod tests {
    use super::{report, INCREMENTAL, TRANSCRIPT};
    use serde_json::{json, Value};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tsr_testrunner::baseline::Roots;
    use tsr_testrunner::result::{Outcome, Report, State};

    const ID: &str = "tsc/commandLine/a-scenario.js";

    /// A reference root holding `ID` with `reference`, and an empty local root.
    struct Dirs {
        path: PathBuf,
        roots: Roots,
    }

    impl Dirs {
        fn new(reference: &[u8]) -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "phase4-tsctests-suite-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let roots = Roots::new(path.join("reference"), path.join("local"));
            let file = roots.reference.join(ID);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, reference).unwrap();
            Self { path, roots }
        }
    }

    impl Drop for Dirs {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// A row of `row::run_scenario` with `state` merged in.
    fn row(state: &Value) -> Value {
        let mut row = json!({
            "version": 1,
            "id": ID,
            "family": "tsc",
            "baseline": ID,
            "scenario_sha256": "00",
            "progress": {"stage": "done", "commands": 2, "edits_completed": 1, "edits": 1},
            "transcript": {"sha256": "00", "bytes": 3},
        });
        for (key, value) in state.as_object().unwrap() {
            row[key] = value.clone();
        }
        row
    }

    fn lines(report: &Report) -> Vec<String> {
        let mut out = Vec::new();
        report.write(&mut out).unwrap();
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn a_completed_run_matching_its_reference_passes_both_subtests() {
        let dirs = Dirs::new(b"one\n");
        let row = row(&json!({"state": "completed", "sha256": "00", "bytes": 4,
                              "unexpected_diff": null}));
        let report = report(&row, b"one\n", &dirs.roots);
        assert_eq!(
            lines(&report),
            [
                format!(r#"{{"id":"{ID}/{TRANSCRIPT}","state":"pass"}}"#),
                format!(r#"{{"id":"{ID}/{INCREMENTAL}","state":"pass"}}"#),
            ]
        );
        assert!(!dirs.roots.local.exists());
    }

    #[test]
    fn a_differing_transcript_fails_and_lands_under_local() {
        let dirs = Dirs::new(b"one\ntwo\n");
        let row = row(&json!({"state": "completed", "sha256": "00", "bytes": 10,
                              "unexpected_diff": null}));
        let report = report(&row, b"one\nthree\n", &dirs.roots);
        assert_eq!(report.lines.len(), 2);
        assert_eq!(report.lines[0].id, format!("{ID}/{TRANSCRIPT}"));
        assert_eq!(report.lines[0].state, State::Fail);
        let reason = report.lines[0].reason.as_deref().unwrap();
        assert!(reason.contains("has changed"), "{reason}");
        let detail = report.lines[0].detail.as_deref().unwrap();
        assert!(
            detail.contains("-two") && detail.contains("+three"),
            "{detail}"
        );
        assert_eq!(
            std::fs::read(dirs.roots.local.join(ID)).unwrap(),
            b"one\nthree\n"
        );
        assert_eq!(report.lines[1].state, State::Pass);
    }

    #[test]
    fn an_unexpected_incremental_diff_fails_the_incremental_subtest_alone() {
        let dirs = Dirs::new(b"one\n");
        let diff = "Edit [0]:: no change\n!!! Unexpected diff, please review and either fix or write explanation as expectedDiff !!!\n--- nonIncremental /home/src/a.js\n+++ incremental /home/src/a.js\n";
        let row = row(&json!({"state": "completed", "sha256": "00", "bytes": 4,
                              "unexpected_diff": diff}));
        let report = report(&row, b"one\n", &dirs.roots);
        assert_eq!(report.lines[0].state, State::Pass);
        let incremental = &report.lines[1];
        assert_eq!(incremental.id, format!("{ID}/{INCREMENTAL}"));
        assert_eq!(incremental.state, State::Fail);
        assert!(incremental
            .reason
            .as_deref()
            .unwrap()
            .contains("unexpected diff with incremental build"));
        assert_eq!(incremental.detail.as_deref(), Some(diff));
        assert!(!dirs.roots.local.exists());
    }

    #[test]
    fn a_run_that_stopped_fails_both_subtests_with_its_reason_and_compares_nothing() {
        let dirs = Dirs::new(b"one\n");
        let mut panicked = row(&json!({"state": "failed", "class": "panic",
                                       "reason": "index out of bounds", "location": "crates/x/src/y.rs:12"}));
        panicked["progress"] =
            json!({"stage": "edit", "commands": 2, "edits_completed": 1, "edits": 3});
        let report = report(&panicked, b"one\n", &dirs.roots);
        let expected = Outcome::fail_with(
            "panic: index out of bounds",
            "stopped in stage edit after 2 command(s), 1 of 3 edit(s) completed; at crates/x/src/y.rs:12",
        );
        let mut want = Report::default();
        want.subtest(ID, TRANSCRIPT, expected.clone());
        want.subtest(ID, INCREMENTAL, expected);
        assert_eq!(report.lines, want.lines);
        assert!(!dirs.roots.local.exists());

        let harness = row(&json!({"state": "failed", "class": "harness",
                                  "reason": "the runner named another baseline", "location": null}));
        let report = super::report(&harness, b"", &dirs.roots);
        assert!(report.lines.iter().all(|line| line.state == State::Fail
            && line.reason.as_deref() == Some("harness: the runner named another baseline")
            && line.detail.as_deref()
                == Some("stopped in stage done after 2 command(s), 1 of 1 edit(s) completed")));

        let refused = row(&json!({"state": "unsupported", "operation": "watch mode"}));
        let report = super::report(&refused, b"", &dirs.roots);
        assert_eq!(report.lines.len(), 2);
        assert!(report.lines.iter().all(|line| line.state == State::Fail
            && line.reason.as_deref() == Some("unsupported: watch mode")));
    }
}
