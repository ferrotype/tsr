//! The result-line contract with `scripts/parity.py`: one JSON object per
//! sub-test on stdout, `{"id": "<variant>/<subtest>", "state": "pass" |
//! "fail" | "skip", "reason"?, "detail"?}`. A variant id is
//! `<suite>/<configured name>` (`compiler/foo(target=es2015).ts`); a sub-test
//! name is the pin's `t.Run` name with its spaces replaced by hyphens
//! (`sourcemap-record`, `union-ordering`).
use serde::Serialize;
use std::io::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Pass,
    Fail,
    Skip,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ResultLine {
    pub id: String,
    pub state: State,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// How one sub-test ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    Fail {
        reason: String,
        detail: Option<String>,
    },
    Skip {
        reason: String,
    },
}

impl Outcome {
    pub fn fail(reason: impl Into<String>) -> Self {
        Self::Fail {
            reason: reason.into(),
            detail: None,
        }
    }

    pub fn fail_with(reason: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::Fail {
            reason: reason.into(),
            detail: Some(detail.into()),
        }
    }

    pub fn skip(reason: impl Into<String>) -> Self {
        Self::Skip {
            reason: reason.into(),
        }
    }

    pub fn is_pass(&self) -> bool {
        matches!(self, Outcome::Pass)
    }
}

impl From<crate::Stop> for Outcome {
    fn from(stop: crate::Stop) -> Self {
        match stop {
            crate::Stop::Fatal(message) => Outcome::fail(message),
            crate::Stop::Skip(message) => Outcome::skip(message),
        }
    }
}

/// The sub-test results of one variant, in the pin's order.
#[derive(Debug, Default)]
pub struct Report {
    pub lines: Vec<ResultLine>,
}

impl Report {
    /// Records `subtest` of `variant` (`compiler_runner.go` runs them with
    /// `t.Run`), mapping the pin's name to its hyphenated id.
    pub fn subtest(&mut self, variant: &str, subtest: &str, outcome: Outcome) {
        let id = format!("{variant}/{}", subtest.replace(' ', "-"));
        let (state, reason, detail) = match outcome {
            Outcome::Pass => (State::Pass, None, None),
            Outcome::Fail { reason, detail } => (State::Fail, Some(reason), detail),
            Outcome::Skip { reason } => (State::Skip, Some(reason), None),
        };
        self.lines.push(ResultLine {
            id,
            state,
            reason,
            detail,
        });
    }

    /// Writes every line as JSON, one per line.
    pub fn write(&self, out: &mut dyn Write) -> std::io::Result<()> {
        for line in &self.lines {
            serde_json::to_writer(&mut *out, line)?;
            out.write_all(b"\n")?;
        }
        Ok(())
    }
}

/// The variant id of a suite and configured name.
pub fn variant_id(suite: &str, configured_name: &str) -> String {
    format!("{suite}/{configured_name}")
}

#[cfg(test)]
mod tests {
    use super::{Outcome, Report, State};

    #[test]
    fn subtest_ids_hyphenate_the_pinned_names_and_lines_serialize_sparsely() {
        let mut report = Report::default();
        report.subtest("compiler/a.ts", "sourcemap record", Outcome::Pass);
        report.subtest(
            "compiler/a.ts",
            "error",
            Outcome::fail_with("changed", "--- a\n+++ b"),
        );
        report.subtest("compiler/a.ts", "output", Outcome::skip("nondeterministic"));
        assert_eq!(report.lines[0].id, "compiler/a.ts/sourcemap-record");
        assert_eq!(report.lines[1].state, State::Fail);
        let mut out = Vec::new();
        report.write(&mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 3);
        assert_eq!(
            text.lines().next().unwrap(),
            r#"{"id":"compiler/a.ts/sourcemap-record","state":"pass"}"#
        );
        assert!(text
            .lines()
            .nth(1)
            .unwrap()
            .contains(r#""detail":"--- a\n+++ b""#));
    }
}
