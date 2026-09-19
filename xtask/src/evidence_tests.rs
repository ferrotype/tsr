use super::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const PIN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "corsa-evidence-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(dir.join("data")).unwrap();
        fs::create_dir_all(dir.join("status")).unwrap();
        let f = Self(dir);
        f.write(".gitignore", "input.dat\n/target\n");
        f.write("input.dat", "corpus version one");
        f.write("source.txt", "source version one");
        f.write("cases.json", r#"["a","b"]"#);
        f.write("producer.py", "import json, pathlib, sys\np = pathlib.Path\nif p('.git/mutate-source').exists(): p('source.txt').write_text('changed during run')\nprint(json.dumps({'metrics': {'ok': True}}))\nsys.exit(int(p('.git/force-failure').exists()))\n");
        f.set_spec(&RunSpec {
            command: vec!["python3".into(), "producer.py".into()],
            inputs: vec!["input.dat".into()],
            sources: vec!["source.txt".into(), "producer.py".into()],
            target: "host".into(),
            config: "debug".into(),
            cases: None,
        });
        f.write("PORTS.toml", &format!("pin = {PIN:?}\nfile = []\n"));
        f.write(
            "data/go-functions.tsv",
            &format!("# upstream {PIN}\nfile\tpackage\treceiver\tname\tstart\tend\tid\n"),
        );
        let manifest = serde_json::json!({"schema_version":2,"pin":PIN,"ledger_generated_sha256":ledger_generated_hash(&f.0).unwrap(),"inventory_sha256":sha_file(&f.0,"data/go-functions.tsv").unwrap()});
        f.write("data/upstream.json", &manifest.to_string());
        git(&f.0, &["init", "-q"]).unwrap();
        git(&f.0, &["add", "."]).unwrap();
        f.commit();
        f
    }
    fn write(&self, p: &str, text: &str) {
        fs::write(self.0.join(p), text).unwrap();
    }
    fn spec(&self) -> RunSpec {
        specs(&self.0).unwrap().remove("probe").unwrap()
    }
    fn set_spec(&self, spec: &RunSpec) {
        self.write(
            "status/runs.toml",
            &toml::to_string(&BTreeMap::from([("probe", spec)])).unwrap(),
        );
    }
    fn context(&self) -> Context {
        Context::capture_run(&self.0, PIN, &self.spec()).unwrap()
    }
    fn loaded(&self) -> Loaded {
        load(&self.0, &self.context()).unwrap()
    }
    fn commit(&self) {
        git(
            &self.0,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "fixture",
            ],
        )
        .unwrap();
    }
    fn success(&self) {
        assert!(run(&self.0, "probe", PIN).unwrap());
        assert_eq!(self.loaded().states["probe"], "current");
    }
    fn rejected(&self) {
        let r = self.loaded();
        assert_ne!(r.states["probe"], "current");
        assert!(!r.metrics.contains_key("run.probe.ok"));
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn historical_artifacts_must_exist_and_keep_their_identity() {
    let f = Fixture::new();
    f.success();
    let digest = fs::read_to_string(f.0.join("status/evidence/probe.latest")).unwrap();
    let path = format!("status/evidence/{digest}.json");
    let bytes = fs::read(f.0.join(&path)).unwrap();
    let history = serde_json::json!({"evidence_artifacts": {"probe": path}}).to_string();
    f.write("status/history.jsonl", &history);
    validate_history(&f.0).unwrap();
    // Source drift does not invalidate the historical artifact itself.
    f.write("source.txt", "new source");
    validate_history(&f.0).unwrap();
    fs::remove_file(f.0.join(&path)).unwrap();
    assert!(validate_history(&f.0)
        .unwrap_err()
        .contains("history.jsonl:1"));
    f.write(&path, "{}");
    assert!(validate_history(&f.0)
        .unwrap_err()
        .contains("checksum mismatch"));
    fs::write(f.0.join(&path), bytes).unwrap();
    f.write(
        "status/history.jsonl",
        &serde_json::json!({"evidence_artifacts": {"wrong": path}}).to_string(),
    );
    assert!(validate_history(&f.0)
        .unwrap_err()
        .contains("identity/contents mismatch"));
    f.write(
        "status/history.jsonl",
        r#"{"evidence_artifacts":{"probe":"../secret.json"}}"#,
    );
    assert!(validate_history(&f.0)
        .unwrap_err()
        .contains("invalid historical artifact path"));
}

#[test]
fn frozen_manifest_requires_exact_unique_results() {
    let f = Fixture::new();
    let mut spec = f.spec();
    spec.cases = Some("cases.json".into());
    for report in [
        r#"{"metrics":{},"tests":{"a":"pass"}}"#,
        r#"{"metrics":{},"tests":{"a":"pass","b":"pass","c":"pass"}}"#,
        r#"{"metrics":{},"tests":{"a":"pass","b":"unknown"}}"#,
        r#"{"metrics":{},"tests":{"a":"fail","a":"pass","b":"pass"}}"#,
        r#"{"metrics":{"ok":false,"ok":true},"tests":{"a":"pass","b":"pass"}}"#,
    ] {
        assert!(
            report_metrics(&f.0, &spec, report).is_err(),
            "accepted {report}"
        );
    }
    for manifest in [r#"["a","a"]"#, "[]"] {
        f.write("cases.json", manifest);
        assert!(report_metrics(&f.0, &spec, r#"{"metrics":{},"tests":{"a":"pass"}}"#).is_err());
    }
}

#[test]
fn skipped_tests_reduce_parity_and_producers_cannot_forge_derived_metrics() {
    let f = Fixture::new();
    let mut spec = f.spec();
    spec.cases = Some("cases.json".into());
    let m = report_metrics(
        &f.0,
        &spec,
        r#"{"metrics":{"ok":true,"count":1},"tests":{"a":"pass","b":"skip"}}"#,
    )
    .unwrap();
    assert!(matches!(m["ok"], Metric::Bool(true)));
    assert!(matches!(m["count"], Metric::Num(n) if n == 1.0));
    assert!(matches!(m["parity"], Metric::Num(n) if n == 0.5));
    assert!(matches!(m["tests_skipped"], Metric::Num(n) if n == 1.0));
    for key in [
        "parity",
        "tests_total",
        "tests_passed",
        "tests_failed",
        "tests_skipped",
    ] {
        let report = serde_json::json!({"metrics":{key:1},"tests":{"a":"pass","b":"pass"}});
        assert!(report_metrics(&f.0, &spec, &report.to_string()).is_err());
    }
    assert!(report_metrics(&f.0, &f.spec(), r#"{"metrics":{"ok":"true"}}"#).is_err());
}

#[test]
fn source_config_and_ignored_declared_inputs_invalidate_evidence() {
    let f = Fixture::new();
    f.success();
    let original_context = f.context();
    f.write("source.txt", "source version two");
    f.rejected();
    f.write("source.txt", "source version one");
    let original_spec = f.spec();
    let mut changed_spec = original_spec.clone();
    changed_spec.config = "release".into();
    f.set_spec(&changed_spec);
    f.rejected();
    f.set_spec(&original_spec);
    f.write("input.dat", "corpus version two");
    assert!(
        original_context.same_inputs(&f.context()),
        "ignored input should be checked through its declared digest"
    );
    f.rejected();
    f.write("input.dat", "corpus version one");
    assert_eq!(f.loaded().states["probe"], "current");
}

#[test]
fn source_identical_commit_remains_current_but_artifact_corruption_does_not() {
    let f = Fixture::new();
    f.success();
    let before = f.context();
    f.commit();
    let after = f.context();
    assert_ne!(before.revision, after.revision);
    assert!(before.same_inputs(&after));
    let loaded = f.loaded();
    assert_eq!(loaded.states["probe"], "current");
    let artifact = &loaded.artifacts["probe"];
    let bytes = fs::read(f.0.join(artifact)).unwrap();
    fs::write(f.0.join(artifact), [bytes.as_slice(), b"\n"].concat()).unwrap();
    assert!(f.loaded().states["probe"].contains("checksum"));
    f.rejected();
}

#[test]
fn failed_rerun_and_incomplete_attempt_revoke_previous_success() {
    let f = Fixture::new();
    f.success();
    f.write(".git/force-failure", "1");
    assert!(!run(&f.0, "probe", PIN).unwrap());
    f.rejected();
    fs::remove_file(f.0.join(".git/force-failure")).unwrap();
    f.success();
    fs::remove_file(f.0.join("input.dat")).unwrap();
    assert!(run(&f.0, "probe", PIN).is_err());
    f.write("input.dat", "corpus version one");
    f.rejected();
}

#[test]
fn source_mutation_during_a_successful_producer_is_rejected() {
    let f = Fixture::new();
    f.success();
    f.write(".git/mutate-source", "1");
    assert!(!run(&f.0, "probe", PIN).unwrap());
    f.write("source.txt", "source version one");
    f.rejected();
}

#[test]
fn held_run_lock_refuses_another_attempt_and_preserves_its_pointer() {
    let f = Fixture::new();
    f.success();
    let latest = f.0.join("status/evidence/probe.latest");
    let before = fs::read(&latest).unwrap();
    let lock = f.0.join("status/evidence/probe.lock");
    fs::create_dir(&lock).unwrap();
    assert!(run(&f.0, "probe", PIN).unwrap_err().contains("lock"));
    assert_eq!(fs::read(&latest).unwrap(), before);
    fs::remove_dir(&lock).unwrap();
    f.success();
    assert!(
        !lock.exists(),
        "successful completion must release its lock"
    );
}

#[test]
fn docs_policy_and_unrelated_run_edits_reuse_existing_measurements() {
    let f = Fixture::new();
    f.success();
    for dir in ["docs", "sprints"] {
        fs::create_dir_all(f.0.join(dir)).unwrap();
    }
    f.write("PLAN.md", "A documentation-only change\n");
    f.write("docs/design.md", "A design note\n");
    f.write("sprints/S02.toml", "exit = []\n");
    f.write("status/experiments.toml", "# revised acceptance policy\n");
    let original = fs::read_to_string(f.0.join("status/runs.toml")).unwrap();
    let mut other = f.spec();
    other.config = "unrelated run".into();
    f.write(
        "status/runs.toml",
        &(original + &toml::to_string(&BTreeMap::from([("other", other)])).unwrap()),
    );
    assert_eq!(f.loaded().states["probe"], "current");
    assert_eq!(f.loaded().states["other"], "missing");
}

#[test]
fn explicit_document_inputs_and_source_selection_changes_invalidate_only_their_run() {
    let f = Fixture::new();
    f.write("PLAN.md", "consumed document version one");
    let mut spec = f.spec();
    spec.inputs.push("PLAN.md".into());
    f.set_spec(&spec);
    f.success();
    f.write("PLAN.md", "consumed document version two");
    f.rejected();
    f.success();
    spec.sources.push("PLAN.md".into());
    f.set_spec(&spec);
    f.rejected();
    f.success();
    f.write("PLAN.md", "consumed document version three");
    f.rejected();
}

#[test]
fn source_globs_track_additions_deletions_and_selected_dependencies() {
    let f = Fixture::new();
    fs::create_dir_all(f.0.join("crates/one/src")).unwrap();
    fs::create_dir_all(f.0.join("crates/two/src")).unwrap();
    f.write("crates/one/src/lib.rs", "one");
    f.write("crates/two/src/lib.rs", "two");
    let mut spec = f.spec();
    spec.sources.push("crates/one/**".into());
    f.set_spec(&spec);
    f.success();
    f.write("crates/two/src/lib.rs", "unrelated dependency changed");
    assert_eq!(f.loaded().states["probe"], "current");
    f.write("crates/one/src/new.rs", "new untracked selected code");
    f.rejected();
    fs::remove_file(f.0.join("crates/one/src/new.rs")).unwrap();
    assert_eq!(f.loaded().states["probe"], "current");
    git(&f.0, &["add", "crates/one"]).unwrap();
    f.commit();
    fs::remove_file(f.0.join("crates/one/src/lib.rs")).unwrap();
    f.rejected();
}

#[test]
fn generation_evidence_tracks_generator_inputs_but_not_unconsumed_leaf_bodies() {
    let f = Fixture::new();
    let mut registered: BTreeMap<String, RunSpec> =
        toml::from_str(include_str!("../../status/runs.toml")).unwrap();
    let generator = registered.remove("gen").unwrap();
    // Exercise the real source/input declaration with the tiny fixture producer;
    // frontend execution is independent of whether evidence becomes stale.
    let mut spec = f.spec();
    spec.sources = generator.sources;
    spec.inputs = generator.inputs;
    f.set_spec(&spec);
    for input in &spec.inputs {
        let path = f.0.join(input);
        if !path.exists() {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            f.write(input, "original input");
        }
    }
    let leaf_bodies = [
        "crates/tsr_arena/src/arena.rs",
        "crates/tsr_jsstring/src/jsstring.rs",
    ];
    let consumed = [
        "crates/tsr_ast/src/data_generated.rs",
        "crates/tsr_diagnostics/src/generated.rs",
        "crates/tsr_encoder/src/generated.rs",
        "crates/tsr_arena/Cargo.toml",
        "crates/tsr_jsstring/Cargo.toml",
        "xtask/src/gen/ast.rs",
        "tools/s03/ast-export.mts",
        "scripts/s03.py",
        "scripts/s04_common.py",
        "scripts/s04_runtime.py",
        "data/s03/schema/ast.json",
        "data/s03/generated.json",
        "data/s03/api-special-codecs.json",
        "data/s06/generated-ast-scope.json",
        "data/s04/toolchains.toml",
        "Cargo.toml",
        "Cargo.lock",
        ".cargo/config.toml",
        "rust-toolchain.toml",
        "rustfmt.toml",
        "upstream",
    ];
    for path in leaf_bodies.into_iter().chain(consumed) {
        fs::create_dir_all(f.0.join(path).parent().unwrap()).unwrap();
        f.write(path, "original input");
    }
    f.success();
    for path in leaf_bodies {
        f.write(path, "unconsumed implementation changed");
        assert_eq!(f.loaded().states["probe"], "current", "{path}");
    }
    for path in consumed {
        f.write(path, "consumed input changed");
        f.rejected();
        f.write(path, "original input");
        assert_eq!(f.loaded().states["probe"], "current", "{path}");
    }
}

#[test]
fn default_source_set_ignores_docs_but_includes_build_configuration() {
    let f = Fixture::new();
    let mut spec = f.spec();
    spec.sources = default_sources();
    f.set_spec(&spec);
    fs::create_dir_all(f.0.join(".cargo")).unwrap();
    f.write(".cargo/config.toml", "# configuration one\n");
    f.success();
    f.write("PLAN.md", "documentation only\n");
    assert_eq!(f.loaded().states["probe"], "current");
    f.write(".cargo/config.toml", "# configuration two\n");
    f.rejected();
}

#[test]
fn dirty_dependency_invalidates_only_runs_that_select_it() {
    let f = Fixture::new();
    let dependency = f.0.join("dependency");
    fs::create_dir(&dependency).unwrap();
    git(&dependency, &["init", "-q"]).unwrap();
    fs::write(dependency.join("code.txt"), "original").unwrap();
    git(&dependency, &["add", "."]).unwrap();
    git(
        &dependency,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "dependency",
        ],
    )
    .unwrap();
    git(&f.0, &["add", "dependency"]).unwrap();
    let mut dependent = f.spec();
    dependent.sources = vec!["dependency".into()];
    let specs_text = toml::to_string(&BTreeMap::from([
        ("probe", f.spec()),
        ("dependent", dependent),
    ]))
    .unwrap();
    f.write("status/runs.toml", &specs_text);
    f.success();
    assert!(run(&f.0, "dependent", PIN).unwrap());
    fs::write(dependency.join("code.txt"), "dirty").unwrap();
    let report_context = Context::capture(&f.0, PIN).unwrap();
    let result = load(&f.0, &report_context).unwrap();
    assert_eq!(result.states["probe"], "current");
    assert!(result.states["dependent"].contains("dirty gitlink"));
    assert!(!result.metrics.contains_key("run.dependent.ok"));
}
