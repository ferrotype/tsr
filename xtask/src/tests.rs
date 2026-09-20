use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "tracker-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&dir).unwrap();
        for p in ["data", "status", "sprints", "crates/demo", "docs/adr"] {
            fs::create_dir_all(dir.join(p)).unwrap();
        }
        let f = Self(dir);
        f.write(
            "PORTS.toml",
            &format!(
                r#"pin = "{PIN}"
[[file]]
go = "tsc/demo.go"
package = "demo"
crate = "demo"
phase = 0
kind = "source"
status = "ported"
rust = ["crates/demo/lib.rs"]
verify = ["run.proof.ok == true"]
pin = "{PIN}"
source_hash = "{}"
loc = 10
"#,
                "b".repeat(64)
            ),
        );
        f.write(
            "crates/demo/lib.rs",
            "// port: tsc/demo.go:A.Map\npub fn map() {}\n",
        );
        f.write("data/go-functions.tsv", &format!("# upstream {PIN}\nfile\tpackage\treceiver\tname\tstart\tend\tid\ntsc/demo.go\tdemo\t*A\tMap\t1\t2\ttsc/demo.go:A.Map\ntsc/demo.go\tdemo\t*B\tMap\t3\t4\ttsc/demo.go:B.Map\n"));
        f.write(
            "status/experiments.toml",
            r#"[E7]
title = "WASM"
nature = "fixture"
[[E7.criteria]]
id = "size"
metric = "run.proof.size"
op = "<="
threshold = 0.25
[[E7.criteria]]
id = "checker"
metric = "run.proof.checker"
op = "=="
threshold = true
"#,
        );
        f.write(
            "status/runs.toml",
            r#"[proof]
command = ["python3", "producer.py"]
inputs = ["producer.py"]
target = "host"
config = "test"
"#,
        );
        f.write("producer.py", "import json\nprint(json.dumps({'metrics': {'ok': True, 'size': 0.2, 'checker': False}}))\n");
        f.write(
            "sprints/S01.toml",
            r#"id = "S01"
title = "fixture"
exit = ["ledger.files_total > 0"]
[[item]]
id = "required"
title = "Required missing result"
done_when = ["run.proof.missing == true"]
"#,
        );
        f.manifest();
        f.git(&["init", "--quiet"]);
        f.git(&["add", "."]);
        f.git(&[
            "-c",
            "user.name=Tracker Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "fixture",
        ]);
        f
    }
    fn write(&self, path: &str, text: &str) {
        fs::write(self.0.join(path), text).unwrap();
    }
    fn replace(&self, path: &str, from: &str, to: &str) {
        self.write(
            path,
            &fs::read_to_string(self.0.join(path))
                .unwrap()
                .replace(from, to),
        );
    }
    fn git(&self, args: &[&str]) {
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(&self.0)
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    }
    fn manifest(&self) {
        let value = serde_json::json!({"schema_version":2,"pin":PIN,"ledger_generated_sha256":evidence::ledger_generated_hash(&self.0).unwrap(),"inventory_sha256":evidence::hash(&fs::read(self.0.join("data/go-functions.tsv")).unwrap())});
        self.write("data/upstream.json", &value.to_string());
    }
    fn report(&self) -> Report {
        build_report(&self.0)
    }
    fn render_views(&self, report: &Report) {
        write_status_md(&self.0, report);
        write_status_json(&self.0, report);
        write_dashboard(&self.0, report);
    }
    fn archive(&self) {
        assert!(evidence::run(&self.0, "proof", PIN).unwrap());
        let mut report = self.report();
        report.generated = "2001-02-03".into();
        self.write("status/history.jsonl", "");
        self.render_views(&report);
    }
    fn view_bytes(&self) -> BTreeMap<String, Vec<u8>> {
        [
            "STATUS.md",
            "status/status.json",
            "docs/status.html",
            "status/unmapped-functions.json",
            "status/history.jsonl",
        ]
        .into_iter()
        .map(|path| (path.into(), fs::read(self.0.join(path)).unwrap()))
        .collect()
    }
    fn rewrite_artifact(&self, edit: impl FnOnce(&mut serde_json::Value)) {
        let path = self.report().evidence_artifacts["proof"].clone();
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(self.0.join(path)).unwrap()).unwrap();
        edit(&mut value);
        let bytes = serde_json::to_vec_pretty(&value).unwrap();
        let digest = evidence::hash(&bytes);
        fs::write(self.0.join(format!("status/evidence/{digest}.json")), bytes).unwrap();
        self.write("status/evidence/proof.latest", &digest);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
const PIN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[test]
fn receiver_ids_count_distinct_mappings_and_reach_full_coverage() {
    let f = Fixture::new();
    let report = f.report();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(num(&report.metrics, "functions.total"), 2.0);
    assert_eq!(num(&report.metrics, "functions.ratio"), 0.5);
    f.write(
        "crates/demo/lib.rs",
        "// port: tsc/demo.go:A.Map\n// port: tsc/demo.go:B.Map\n",
    );
    assert_eq!(num(&f.report().metrics, "functions.ratio"), 1.0);
}
#[test]
fn manual_verified_and_nonexistent_rust_cannot_count_as_verified() {
    let f = Fixture::new();
    f.replace("PORTS.toml", "status = \"ported\"", "status = \"verified\"");
    f.manifest();
    let r = f.report();
    assert_eq!(num(&r.metrics, "ledger.files_verified"), 0.0);
    assert!(!r.errors.is_empty());
    f.replace("PORTS.toml", "status = \"verified\"", "status = \"ported\"");
    fs::remove_file(f.0.join("crates/demo/lib.rs")).unwrap();
    f.manifest();
    let r = f.report();
    assert_eq!(num(&r.metrics, "ledger.files_verified"), 0.0);
    assert!(!r.errors.is_empty());
}
#[test]
fn complete_evidence_is_required_for_verification_and_experiments() {
    let f = Fixture::new();
    assert_eq!(num(&f.report().metrics, "ledger.files_verified"), 0.0);
    assert!(evidence::run(&f.0, "proof", PIN).unwrap());
    let r = f.report();
    assert_eq!(num(&r.metrics, "ledger.files_verified"), 1.0);
    assert_eq!(num(&r.metrics, "exp.E7.size.pass"), 1.0);
    assert_eq!(num(&r.metrics, "exp.E7.checker.pass"), 0.0);
    assert_eq!(num(&r.metrics, "exp.E7.pass"), 0.0);
    assert!(!r.sprints[0].done);
    f.replace("producer.py", "'checker': False", "'checker': True");
    assert_eq!(num(&f.report().metrics, "ledger.files_verified"), 0.0);
    assert!(evidence::run(&f.0, "proof", PIN).unwrap());
    assert_eq!(num(&f.report().metrics, "exp.E7.pass"), 1.0);
}

#[test]
fn scanner_evidence_does_not_complete_e4_or_ast_diagnostic_integration() {
    let f = Fixture::new();
    f.write(
        "status/experiments.toml",
        include_str!("../../status/experiments.toml"),
    );
    let ledger: Ledger = toml::from_str(include_str!("../../PORTS.toml")).unwrap();
    let ast_checks = &ledger
        .file
        .iter()
        .find(|file| file.go == "tsc/internal/ast/diagnostic.go")
        .unwrap()
        .verify;
    f.replace(
        "PORTS.toml",
        "verify = [\"run.proof.ok == true\"]",
        &format!("verify = {}", serde_json::to_string(ast_checks).unwrap()),
    );
    f.replace("status/runs.toml", "[proof]", "[scanner]");
    let mut runs = fs::read_to_string(f.0.join("status/runs.toml")).unwrap();
    runs.push_str("\n[e1]\ncommand = [\"python3\", \"e1.py\"]\ninputs = [\"e1.py\"]\ntarget = \"host\"\nconfig = \"test\"\n");
    f.write("status/runs.toml", &runs);
    f.write("e1.py", "print('{\"metrics\":{\"parity\":1}}')\n");
    f.write(
        "producer.py",
        &format!(
            "print({:?})\n",
            serde_json::json!({"metrics": {
                "parity": 1, "regexp_parity": 1, "rescan_parity": 1,
                "diagnostics": true, "token_value_bytes": true, "table_current": true
            }})
            .to_string()
        ),
    );
    f.manifest();
    assert!(evidence::run(&f.0, "scanner", PIN).unwrap());
    assert!(evidence::run(&f.0, "e1", PIN).unwrap());
    let report = f.report();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(num(&report.metrics, "exp.E4.diagnostics.pass"), 1.0);
    assert_eq!(num(&report.metrics, "exp.E4.token_value_bytes.pass"), 1.0);
    assert_eq!(num(&report.metrics, "exp.E4.pass"), 0.0);
    assert_eq!(num(&report.metrics, "ledger.files_verified"), 0.0);
    assert!(!report.metrics.contains_key("run.e4.diagnostics"));
    assert_eq!(
        eval_check(&report.metrics, "run.e1.parity >= 0.999"),
        Some(true)
    );
}

#[test]
fn s04_instrumentation_does_not_complete_s09_all_scenarios_item() {
    let f = Fixture::new();
    f.write("sprints/S09.toml", include_str!("../../sprints/S09.toml"));
    f.write(
        "status/experiments.toml",
        include_str!("../../status/experiments.toml"),
    );
    f.replace("status/runs.toml", "[proof]", "[e3]");
    let mut measured = serde_json::json!({
        "miri": true, "address_sanitizer": true,
        "live_owner_delta": 0, "live_allocation_delta": 0,
        "id_exhaustion": true, "wrong_owner_rejected": true,
        "stale_and_recycled_ids_rejected": true,
        "concurrent_lazy_storage": true, "mapper_bundle_disposal": true
    });
    let capture = |metrics: &serde_json::Value| {
        f.write(
            "producer.py",
            &format!(
                "print({:?})\n",
                serde_json::json!({"metrics": metrics}).to_string()
            ),
        );
        assert!(evidence::run(&f.0, "e3", PIN).unwrap());
        f.report()
    };
    let item_result = |report: &Report| {
        report
            .sprints
            .iter()
            .find(|s| s.id == "S09")
            .unwrap()
            .items
            .iter()
            .find(|(id, _, _)| id == "S09-5")
            .unwrap()
            .2
    };
    let partial = capture(&measured);
    assert!(partial.errors.is_empty(), "{:?}", partial.errors);
    assert_eq!(num(&partial.metrics, "exp.E3.miri.pass"), 1.0);
    assert_eq!(num(&partial.metrics, "exp.E3.pass"), 0.0);
    assert_ne!(item_result(&partial), Some(true));

    for criterion in &read_experiments(&f.0)["E3"].criteria {
        assert_eq!(criterion.op, "==");
        measured[criterion.metric.strip_prefix("run.e3.").unwrap()] = criterion.threshold.clone();
    }
    let complete = capture(&measured);
    assert_eq!(num(&complete.metrics, "exp.E3.pass"), 1.0);
    assert_eq!(item_result(&complete), Some(true));
}

#[test]
fn encoder_capture_routes_only_encoder_criteria_and_does_not_verify_decoder() {
    let f = Fixture::new();
    f.write(
        "status/experiments.toml",
        include_str!("../../status/experiments.toml"),
    );
    let ledger: Ledger = toml::from_str(include_str!("../../PORTS.toml")).unwrap();
    let decoder = ledger
        .file
        .iter()
        .find(|file| file.go == "tsc/internal/api/encoder/decoder.go")
        .unwrap();
    f.replace(
        "PORTS.toml",
        "verify = [\"run.proof.ok == true\"]",
        &format!(
            "verify = {}",
            serde_json::to_string(&decoder.verify).unwrap()
        ),
    );
    f.replace("status/runs.toml", "[proof]", "[e1]");
    let mut measured = serde_json::json!({
        "parity": 1, "frozen_denominator": true,
        "encoder_success_error": true, "encoder_output_bytes": true
    });
    let capture = |metrics: &serde_json::Value| {
        f.write(
            "producer.py",
            &format!(
                "print({:?})\n",
                serde_json::json!({"metrics": metrics}).to_string()
            ),
        );
        assert!(evidence::run(&f.0, "e1", PIN).unwrap());
        f.report()
    };
    f.manifest();
    let encoded = capture(&measured);
    assert!(encoded.errors.is_empty(), "{:?}", encoded.errors);
    assert_eq!(
        num(&encoded.metrics, "exp.E4.encoder_success_error.pass"),
        1.0
    );
    assert_eq!(
        num(&encoded.metrics, "exp.E4.encoder_output_bytes.pass"),
        1.0
    );
    assert_eq!(num(&encoded.metrics, "exp.E4.pass"), 0.0);
    assert_eq!(num(&encoded.metrics, "ledger.files_verified"), 0.0);
    assert!(!encoded.metrics.contains_key("run.e4.diagnostics"));
    assert!(!encoded.metrics.contains_key("run.e4.token_literal_bytes"));

    measured["decoder_parity"] = false.into();
    assert_eq!(
        num(&capture(&measured).metrics, "ledger.files_verified"),
        0.0
    );
    measured["decoder_parity"] = true.into();
    assert_eq!(
        num(&capture(&measured).metrics, "ledger.files_verified"),
        0.0
    );
    measured["decoder_watchdog"] = false.into();
    assert_eq!(
        num(&capture(&measured).metrics, "ledger.files_verified"),
        0.0
    );
    measured["decoder_watchdog"] = true.into();
    assert_eq!(
        num(&capture(&measured).metrics, "ledger.files_verified"),
        1.0
    );
}

#[test]
fn s06_requires_each_supplemental_result_even_when_primary_evidence_passes() {
    let sprint: Sprint = toml::from_str(include_str!("../../sprints/S06.toml")).unwrap();
    let mut metrics = Metrics::new();
    // Isolate supplemental acceptance from unrelated prerequisite failures.
    for check in sprint
        .exit
        .iter()
        .chain(sprint.item.iter().flat_map(|i| &i.done_when))
    {
        let fields: Vec<_> = check.split_whitespace().collect();
        let value = if fields[2] == "true" {
            Metric::Bool(true)
        } else {
            Metric::Num(1.0)
        };
        metrics.insert(fields[0].to_owned(), value);
    }
    let passes = |checks: &[String], metrics: &Metrics| {
        checks
            .iter()
            .all(|check| eval_check(metrics, check) == Some(true))
    };
    assert!(passes(&sprint.exit, &metrics));
    for (metric, item_id) in [
        ("run.e1.depth", "S06-2"),
        ("run.e1.parser_regressions", "S06-2"),
        ("run.e1.ast_runtime", "S06-3"),
        ("run.e1.ast_utilities", "S06-3"),
        ("run.e3.ast_runtime", "S06-3"),
        ("run.e1.decoder_parity", "S06-4"),
        ("run.e1.decoder_watchdog", "S06-4"),
    ] {
        let item = sprint.item.iter().find(|item| item.id == item_id).unwrap();
        metrics.remove(metric);
        assert!(
            !passes(&sprint.exit, &metrics),
            "missing {metric} closed S06"
        );
        assert!(
            !passes(&item.done_when, &metrics),
            "missing {metric} closed {item_id}"
        );
        metrics.insert(metric.into(), Metric::Bool(false));
        assert!(
            !passes(&sprint.exit, &metrics),
            "failed {metric} closed S06"
        );
        assert!(
            !passes(&item.done_when, &metrics),
            "failed {metric} closed {item_id}"
        );
        metrics.insert(metric.into(), Metric::Bool(true));
        assert!(passes(&sprint.exit, &metrics));
    }
}
#[test]
fn stale_sync_pin_and_missing_verification_checks_do_not_pass() {
    let f = Fixture::new();
    f.replace(
        "PORTS.toml",
        &format!("pin = \"{PIN}\"\nsource_hash"),
        "pin = \"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\"\nsource_hash",
    );
    f.manifest();
    assert!(evidence::run(&f.0, "proof", PIN).unwrap());
    let r = f.report();
    assert_eq!(num(&r.metrics, "ledger.files_stale"), 1.0);
    assert_eq!(num(&r.metrics, "ledger.files_verified"), 0.0);
}
#[test]
fn required_unchecked_or_failing_items_block_but_optional_items_do_not() {
    let f = Fixture::new();
    assert!(!f.report().sprints[0].done);
    f.replace(
        "sprints/S01.toml",
        "done_when = [\"run.proof.missing == true\"]",
        "",
    );
    assert!(!f.report().sprints[0].done);
    f.replace(
        "sprints/S01.toml",
        "title = \"Required missing result\"",
        "title = \"Optional research\"\nrequired = false",
    );
    assert!(f.report().sprints[0].done);
}
#[test]
fn unknown_markers_and_duplicate_sprint_ids_block_check() {
    let f = Fixture::new();
    f.replace(
        "sprints/S01.toml",
        "done_when = [\"run.proof.missing == true\"]",
        "required = false",
    );
    f.write("crates/demo/lib.rs", "// port: tsc/demo.go:Missing\n");
    assert!(!f.report().sprints[0].done);
    f.write("crates/demo/lib.rs", "");
    fs::copy(f.0.join("sprints/S01.toml"), f.0.join("sprints/S02.toml")).unwrap();
    let r = f.report();
    assert!(r.errors.iter().any(|e| e.contains("duplicate sprint ID")));
    assert!(r.sprints.iter().all(|s| !s.done));
}
#[test]
fn schema_rejects_misspelled_items_and_legacy_scalar_measurements() {
    assert!(toml::from_str::<Sprint>("id='S'\ntitle='x'\n[[items]]\nid='x'\n").is_err());
    assert!(toml::from_str::<Experiment>(
        "title='x'\nnature='x'\nmeasured=1\nop='=='\nthreshold=1\n"
    )
    .is_err());
}
#[test]
fn duplicate_history_publication_does_not_create_new_measurements() {
    let f = Fixture::new();
    let r = f.report();
    record_history(&f.0, &r);
    record_history(&f.0, &r);
    assert_eq!(
        fs::read_to_string(f.0.join("status/history.jsonl"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}
#[test]
fn metadata_only_commit_does_not_invalidate_identical_tested_content() {
    let f = Fixture::new();
    assert!(evidence::run(&f.0, "proof", PIN).unwrap());
    record_history(&f.0, &f.report());
    f.git(&["add", "status/evidence"]);
    f.git(&[
        "-c",
        "user.name=Tracker Test",
        "-c",
        "user.email=test@example.invalid",
        "commit",
        "--quiet",
        "-m",
        "record evidence",
    ]);
    assert_eq!(num(&f.report().metrics, "ledger.files_verified"), 1.0);
    record_history(&f.0, &f.report());
    assert_eq!(
        fs::read_to_string(f.0.join("status/history.jsonl"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}

#[test]
fn complete_parity_requires_exact_equality() {
    let metrics = BTreeMap::from([("run.corpus.parity".into(), Metric::Num(1.0 - 1e-10))]);
    assert_eq!(eval_check(&metrics, "run.corpus.parity == 1"), Some(false));
}

#[test]
fn editable_ledger_state_and_policy_changes_reuse_current_evidence() {
    let f = Fixture::new();
    f.replace("PORTS.toml", "status = \"ported\"", "status = \"planned\"");
    assert!(evidence::provenance(&f.0, PIN).is_ok());
    assert!(evidence::run(&f.0, "proof", PIN).unwrap());
    let artifact = f.report().evidence_artifacts["proof"].clone();
    f.replace("PORTS.toml", "status = \"planned\"", "status = \"ported\"");
    assert!(evidence::provenance(&f.0, PIN).is_ok());
    let r = f.report();
    assert_eq!(r.evidence_artifacts["proof"], artifact);
    assert_eq!(num(&r.metrics, "ledger.files_verified"), 1.0);
    f.replace(
        "PORTS.toml",
        "run.proof.ok == true",
        "run.proof.checker == true",
    );
    assert!(evidence::provenance(&f.0, PIN).is_ok());
    let r = f.report();
    assert_eq!(r.evidence_states["proof"], "current");
    assert_eq!(num(&r.metrics, "ledger.files_verified"), 0.0);
    f.replace("PORTS.toml", "loc = 10", "loc = 11");
    assert!(evidence::provenance(&f.0, PIN).is_err());
}

#[test]
fn generated_projection_has_the_same_canonical_hash_as_python() {
    let f = Fixture::new();
    f.write(
        "PORTS.toml",
        &format!(
            r#"pin = "{PIN}"
[[file]]
go = "tsc/internal/é.go"
package = "internal"
crate = "tsr_core"
phase = 0
kind = "source"
pin = "{PIN}"
source_hash = "{}"
loc = 2
status = "planned"
rust = []
verify = []
"#,
            "b".repeat(64)
        ),
    );
    assert_eq!(
        evidence::ledger_generated_hash(&f.0).unwrap(),
        "637f37b9dba865420809290bc99927d2ab80f1b50231c1ed8a3445a24ab9b528"
    );
}

#[test]
fn history_records_policy_state_changes_without_relabeling_old_measurements() {
    let f = Fixture::new();
    assert!(evidence::run(&f.0, "proof", PIN).unwrap());
    let first = f.report();
    record_history(&f.0, &first);
    f.replace(
        "PORTS.toml",
        "status = \"ported\"",
        "status = \"in-progress\"",
    );
    let second = f.report();
    assert_eq!(first.evidence_artifacts, second.evidence_artifacts);
    assert_eq!(second.evidence_states["proof"], "current");
    record_history(&f.0, &second);
    let entries: Vec<serde_json::Value> = fs::read_to_string(f.0.join("status/history.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(entries.len(), 2);
    assert_eq!(
        entries[0]["evidence_artifacts"],
        entries[1]["evidence_artifacts"]
    );
    assert_ne!(entries[0]["files_verified"], entries[1]["files_verified"]);
}

#[test]
fn archived_views_preserve_recorded_date_and_revision_without_writing() {
    let f = Fixture::new();
    f.archive();
    let before = f.view_bytes();
    let saved: serde_json::Value = serde_json::from_slice(&before["status/status.json"]).unwrap();
    f.git(&[
        "-c",
        "user.name=Tracker Test",
        "-c",
        "user.email=test@example.invalid",
        "commit",
        "--allow-empty",
        "--quiet",
        "-m",
        "new revision with identical sources",
    ]);
    assert_ne!(
        saved["context"]["revision"].as_str().unwrap(),
        f.report().context.unwrap().revision
    );
    assert_eq!(saved["generated"], "2001-02-03");
    assert!(check_committed_views(&f.0).is_ok());
    assert_eq!(f.view_bytes(), before);
}

#[test]
fn archived_execution_provenance_cannot_approve_a_live_metric_check() {
    for changed in ["environment", "host", "toolchain"] {
        let f = Fixture::new();
        f.archive();
        let mut context = f.report().context.unwrap();
        match changed {
            "environment" => {
                context.environment.insert(
                    "RUSTUP_TOOLCHAIN".into(),
                    "archived-cross-host-test-toolchain".into(),
                );
            }
            "host" => context.host = "different-host-architecture".into(),
            "toolchain" => context.toolchain = "different rustc -Vv output".into(),
            _ => unreachable!(),
        }
        f.rewrite_artifact(|record| {
            record["context"]["environment"] = serde_json::json!(context.environment);
            record["context"]["host"] = serde_json::json!(context.host);
            record["host"] = serde_json::json!(context.host);
            record["context"]["toolchain"] = serde_json::json!(context.toolchain);
            record["toolchain"] = serde_json::json!(context.toolchain);
        });
        let metadata = ViewMetadata {
            context,
            generated: "2001-02-03".into(),
        };
        let archived = build_report_in_context(&f.0, Some(&metadata));
        assert_eq!(archived.evidence_states["proof"], "current");
        f.render_views(&archived);
        let before = f.view_bytes();
        assert!(check_committed_views(&f.0).is_ok());
        assert_eq!(f.view_bytes(), before);
        let live = f.report();
        assert!(
            !checks_pass(&live, &["run.proof.ok == true".into()]),
            "accepted foreign {changed}"
        );
        assert!(!live.metrics.contains_key("run.proof.ok"));
        if changed != "environment" {
            assert_eq!(
                live.evidence_states["proof"],
                "stale: host or Rust toolchain changed"
            );
        }
    }
}

#[test]
fn archived_views_preserve_stale_host_and_toolchain_states() {
    for changed in ["host", "toolchain"] {
        let f = Fixture::new();
        f.archive();
        let foreign = format!("foreign {changed}");
        f.rewrite_artifact(|record| {
            record[changed] = serde_json::json!(foreign);
            record["context"][changed] = serde_json::json!(foreign);
        });
        let report = f.report();
        assert_eq!(
            report.evidence_states["proof"],
            "stale: host or Rust toolchain changed"
        );
        f.render_views(&report);
        let before = f.view_bytes();
        assert!(check_committed_views(&f.0).is_ok());
        assert_eq!(f.view_bytes(), before);
        // Changing saved renderer identity must recompute the evidence state,
        // rather than preserve the stale state copied from the summary.
        let mut saved: serde_json::Value =
            serde_json::from_slice(&before["status/status.json"]).unwrap();
        saved["context"][changed] = serde_json::json!(foreign);
        f.write(
            "status/status.json",
            &serde_json::to_string_pretty(&saved).unwrap(),
        );
        assert!(check_committed_views(&f.0).is_err());
    }
}

#[test]
fn old_artifact_contexts_use_their_explicit_execution_identity() {
    let f = Fixture::new();
    f.archive();
    f.rewrite_artifact(|record| {
        let context = record["context"].as_object_mut().unwrap();
        context.remove("host");
        context.remove("toolchain");
    });
    assert!(checks_pass(&f.report(), &["run.proof.ok == true".into()]));
}

#[test]
fn committed_views_recompute_metrics_and_verify_the_separate_worklist() {
    for corrupt_worklist in [false, true] {
        let f = Fixture::new();
        f.archive();
        if corrupt_worklist {
            f.write("status/unmapped-functions.json", "{}");
        } else {
            // Forge all generated views consistently: the checker must replay evidence,
            // rather than deserialize the saved metrics as its rendering input.
            let mut forged = f.report();
            forged.generated = "2001-02-03".into();
            forged
                .metrics
                .insert("run.proof.ok".into(), Metric::Bool(false));
            forged
                .metrics
                .insert("ledger.files_verified".into(), Metric::Num(99.0));
            f.render_views(&forged);
        }
        let before = f.view_bytes();
        assert!(check_committed_views(&f.0).is_err());
        assert_eq!(f.view_bytes(), before);
    }
}

#[test]
fn archived_views_reject_changed_sources_specs_inputs_and_provenance() {
    for changed in ["source", "spec", "input", "provenance"] {
        let f = Fixture::new();
        f.archive();
        match changed {
            "source" => f.replace(
                "crates/demo/lib.rs",
                "pub fn map() {}",
                "pub fn map() { /* changed */ }",
            ),
            "spec" => f.replace(
                "status/runs.toml",
                "config = \"test\"",
                "config = \"release\"",
            ),
            "input" => f.replace(
                "producer.py",
                "import json",
                "import json\n# changed declared input",
            ),
            "provenance" => f.replace("PORTS.toml", "loc = 10", "loc = 11"),
            _ => unreachable!(),
        }
        let before = f.view_bytes();
        assert!(
            check_committed_views(&f.0).is_err(),
            "accepted {changed} drift"
        );
        assert_eq!(f.view_bytes(), before);
    }
}

#[test]
fn archived_views_reject_corrupt_missing_incomplete_and_failed_latest_evidence() {
    for changed in ["corrupt", "missing", "incomplete", "failed"] {
        let f = Fixture::new();
        f.archive();
        let artifact = f.0.join(&f.report().evidence_artifacts["proof"]);
        match changed {
            "corrupt" => fs::write(artifact, "{}").unwrap(),
            "missing" => fs::remove_file(artifact).unwrap(),
            "incomplete" => f.write("status/evidence/proof.latest", "incomplete attempt"),
            "failed" => {
                f.write("producer.py", "import sys\nsys.exit(1)\n");
                assert!(!evidence::run(&f.0, "proof", PIN).unwrap());
            }
            _ => unreachable!(),
        }
        let before = f.view_bytes();
        assert!(
            check_committed_views(&f.0).is_err(),
            "accepted {changed} evidence"
        );
        assert!(!checks_pass(&f.report(), &["run.proof.ok == true".into()]));
        assert_eq!(f.view_bytes(), before);
    }
}

#[test]
fn metric_checks_require_every_requested_check_and_current_valid_evidence() {
    let f = Fixture::new();
    let ok = "run.proof.ok == true".to_string();
    assert!(!checks_pass(&f.report(), std::slice::from_ref(&ok)));
    assert!(evidence::run(&f.0, "proof", PIN).unwrap());
    let mut report = f.report();
    assert!(checks_pass(&report, std::slice::from_ref(&ok)));
    assert!(!checks_pass(&report, &[]));
    for check in [
        "run.proof.checker == true",
        "run.proof.missing == true",
        "malformed",
    ] {
        assert!(!checks_pass(&report, &[ok.clone(), check.into()]));
    }
    report.errors.push("invalid provenance".into());
    assert!(!checks_pass(&report, std::slice::from_ref(&ok)));
    report.errors.clear();
    report.unknown_markers.push("unknown function".into());
    assert!(!checks_pass(&report, std::slice::from_ref(&ok)));
    f.replace(
        "crates/demo/lib.rs",
        "pub fn map() {}",
        "pub fn map() { /* stale */ }",
    );
    assert!(!checks_pass(&f.report(), &[ok]));
}
