//! The shared protocol's stage rule and control kill rule, on a fake oracle.

use crate::jobs::{enter, run_row, BaseRow, Job, Request, RowOutput, Rows, Runner, Stage};
use phase1_mutants::{active, hit, hit_parser, set_tracing};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};

/// Production hits sites 1 (`hit`) and 2 (`hit_parser`); observation hits
/// parser site 3 and production-only sites 1 and 4. The compared stage `stage`
/// digests what production saw as the active mutant (7 and 8 look alike, like
/// a counter-only allocation and its control; 9 differs from its control 10),
/// plus the mutant observation saw and whether site 4 fired there. Stage
/// `other` moves only for 12, the control of 11: it equals its mutant where
/// the mutant differs from the base and differs from it only elsewhere.
struct Fake {
    observe_counts: bool,
}

impl Rows for Fake {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn compared(&self) -> &'static [&'static str] {
        &["stage", "other"]
    }

    fn observe_counts(&self) -> bool {
        self.observe_counts
    }

    fn dumps_kills(&self) -> bool {
        false
    }

    fn request(&self, index: usize, _line: &[u8]) -> Result<Request, String> {
        Ok(Request {
            index,
            id: format!("r{index}"),
            sha256: "s".into(),
            value: Value::Null,
            source_bytes: 0,
        })
    }

    fn execute(&self, _request: &Request, _capture: bool) -> RowOutput {
        enter(Stage::Production);
        let _ = hit(1);
        let _ = hit_parser(2);
        let produced = match active() {
            7 | 8 | 11 | 12 => "counter",
            9 => "mutated",
            _ => "base",
        };
        let other = if active() == 12 { "shifted" } else { "base" };
        enter(Stage::Observe);
        let _ = hit_parser(3);
        let _ = hit(1);
        let fired = if hit(4) { "!" } else { "" };
        let observed = active();
        RowOutput {
            outcomes: BTreeMap::from([
                ("stage".into(), "ok".into()),
                ("other".into(), "ok".into()),
            ]),
            digests: BTreeMap::from([
                ("stage".into(), format!("{produced}/{observed}{fired}")),
                ("other".into(), other.into()),
            ]),
            ..RowOutput::default()
        }
    }
}

fn lines(bytes: &[u8]) -> Vec<Value> {
    String::from_utf8(bytes.to_vec())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

// The mutant switch is process-global, so one test exercises it in order.
#[test]
fn reach_follows_the_stage_rule_and_controls_decide_kills() {
    set_tracing(true);
    let request = Fake {
        observe_counts: false,
    }
    .request(0, b"")
    .unwrap();

    let counts_nothing = run_row(
        &Fake {
            observe_counts: false,
        },
        &request,
        false,
        3,
    );
    assert_eq!(counts_nothing.hits, [1, 2]);
    assert_eq!(
        counts_nothing.observe_hits,
        [1, 3, 4],
        "every site executed while observing is observation reach, reached in production or not"
    );
    assert_eq!(
        counts_nothing.output.digests["stage"], "base/0",
        "the mutant is off while observing"
    );
    let e1_like = run_row(
        &Fake {
            observe_counts: true,
        },
        &request,
        false,
        3,
    );
    assert_eq!(e1_like.hits, [1, 2, 3]);
    assert_eq!(
        e1_like.observe_hits,
        [1, 4],
        "a production-only site executed while observing is observation reach even where parser sites count"
    );
    assert_eq!(
        e1_like.output.digests["stage"], "base/3",
        "lazy parser work while observing keeps the mutant live"
    );
    let production_only = run_row(
        &Fake {
            observe_counts: true,
        },
        &request,
        false,
        4,
    );
    assert_eq!(
        production_only.output.digests["stage"], "base/4",
        "a production-only site never fires while observing"
    );
    assert_eq!(active(), 0, "a row ends with no active mutant");

    set_tracing(false);
    let fake = Fake {
        observe_counts: false,
    };
    let requests = HashMap::from([("r0", &request)]);
    let base = HashMap::from([(
        "r0".to_owned(),
        BaseRow {
            outcomes: BTreeMap::from([
                ("stage".into(), "ok".into()),
                ("other".into(), "ok".into()),
            ]),
            digests: BTreeMap::from([
                ("stage".into(), "base/0".into()),
                ("other".into(), "base".into()),
            ]),
        },
    )]);
    let dump_dir = std::env::temp_dir();
    let runner = Runner {
        rows: &fake,
        requests: &requests,
        base: &base,
        dump_dir: &dump_dir,
    };
    let job = |mutant, control| {
        Job::parse(
            &json!({"mutant": mutant, "control": control, "rows": ["r0", "missing"]}).to_string(),
        )
        .unwrap()
    };
    let mut out = Vec::new();
    runner.job(&job(7, 8), &mut out).unwrap();
    let counter_only = lines(&out);
    let result = counter_only
        .iter()
        .find(|line| line.get("differs").is_some() && line.get("row") == Some(&json!("r0")))
        .unwrap();
    assert_eq!(result["differs"], json!(["stage"]));
    assert_eq!(
        result["kill"], false,
        "the control reproduces the difference"
    );
    assert_eq!(result["control"]["id"], 8);
    assert_eq!(result["control"]["differs"], json!([]));
    assert!(counter_only
        .iter()
        .any(|line| line.get("error").is_some() && line["row"] == "missing"));
    assert_eq!(counter_only.last().unwrap()["differing"], 0);
    assert!(
        counter_only
            .iter()
            .all(|line| line.get("recheck").is_none()),
        "no kill, no recheck"
    );

    let mut out = Vec::new();
    runner.job(&job(9, 10), &mut out).unwrap();
    let real = lines(&out);
    let result = real.iter().find(|line| line.get("kill").is_some()).unwrap();
    assert_eq!(result["kill"], true);
    assert_eq!(result["control"]["digests"]["stage"], "base/0");
    assert_eq!(result["control"]["differs"], json!(["stage"]));
    assert!(real
        .iter()
        .any(|line| line.get("control").is_some() && line.get("heartbeat").is_some()));
    assert!(real
        .iter()
        .any(|line| line.get("recheck") == Some(&json!("r0")) && line["base_ok"] == true));
    assert_eq!(real.last().unwrap()["differing"], 1);

    // 11 differs from the base only in `stage`, where its control 12 equals
    // it; the control differs from it only in `other`, where 11 equals the
    // base. No stage differs from both, so the row is no kill.
    let mut out = Vec::new();
    runner.job(&job(11, 12), &mut out).unwrap();
    let elsewhere = lines(&out);
    let result = elsewhere
        .iter()
        .find(|line| line.get("kill").is_some())
        .unwrap();
    assert_eq!(result["differs"], json!(["stage"]));
    assert_eq!(result["control"]["differs"], json!(["other"]));
    assert_eq!(
        result["kill"], false,
        "the mutant must differ from native and from its control in one stage"
    );
    assert_eq!(elsewhere.last().unwrap()["differing"], 0);

    assert!(Job::parse(r#"{"mutant":1,"rows":[],"extra":1}"#).is_err());
    let defaults = Job::parse(r#"{"mutant":1,"rows":["a"]}"#).unwrap();
    assert_eq!(
        (
            defaults.max_kills,
            defaults.control,
            defaults.recheck,
            defaults.report_all
        ),
        (3, None, true, false)
    );
    set_tracing(true);

    // The trace line carries both reach sets: the excuse rule reads
    // observation reach from it, so a missing field would excuse a home that
    // runs while observing.
    let out = std::env::temp_dir().join(format!(
        "phase1-mutation-trace-{}.ndjson",
        std::process::id()
    ));
    crate::jobs::trace(
        &Fake {
            observe_counts: false,
        },
        std::slice::from_ref(&request),
        &out,
        None,
        None,
    )
    .unwrap();
    let traced = lines(&std::fs::read(&out).unwrap());
    std::fs::remove_file(&out).unwrap();
    assert_eq!(traced.len(), 1);
    assert_eq!(traced[0]["row"], "r0");
    assert_eq!(traced[0]["hits"], json!([1, 2]));
    assert_eq!(traced[0]["observe_hits"], json!([1, 3, 4]));
}
