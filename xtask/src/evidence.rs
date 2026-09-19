//! Run reviewed producers and retain immutable, content-addressed evidence.
use crate::{Metric, Metrics};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

type Result<T> = std::result::Result<T, String>;

pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn read(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}
fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().into());
    }
    Ok(out.stdout)
}
fn current_host() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}
fn current_toolchain() -> Result<String> {
    let output = Command::new("rustc")
        .arg("-Vv")
        .output()
        .map_err(|e| format!("cannot identify Rust toolchain: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cannot identify Rust toolchain: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let toolchain = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;
    if toolchain.trim().is_empty() {
        return Err("cannot identify Rust toolchain: empty rustc -Vv output".into());
    }
    Ok(toolchain)
}
pub fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && Path::new(path)
            .components()
            .all(|p| matches!(p, Component::Normal(_)))
}
fn sha_file(root: &Path, path: &str) -> Result<String> {
    if !safe_path(path) {
        return Err(format!("invalid relative input path: {path}"));
    }
    let p = root.join(path);
    let canonical = p.canonicalize().map_err(|e| format!("{path}: {e}"))?;
    if !canonical.starts_with(root.canonicalize().map_err(|e| e.to_string())?) {
        return Err(format!("input escapes repository: {path}"));
    }
    Ok(hash(&read(&p)?))
}
fn excluded(path: &str) -> bool {
    matches!(
        path,
        "STATUS.md"
            | "docs/status.html"
            | "status/status.json"
            | "status/history.jsonl"
            | "status/unmapped-functions.json"
    ) || path.starts_with("status/evidence/")
        || path.starts_with("target/")
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Context {
    pub revision: String,
    pub source_sha256: String,
    pub upstream_pin: String,
    pub environment: BTreeMap<String, String>,
    // Old run artifacts already retain these fields on Record. Saved views now
    // also need their renderer identity to reproduce stale evidence faithfully.
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub toolchain: String,
}
impl Context {
    pub fn same_inputs(&self, other: &Self) -> bool {
        self.source_sha256 == other.source_sha256
            && self.upstream_pin == other.upstream_pin
            && self.environment == other.environment
    }
    pub fn capture(root: &Path, pin: &str) -> Result<Self> {
        Self::capture_sources(root, pin, &default_sources())
    }
    fn capture_run(root: &Path, pin: &str, spec: &RunSpec) -> Result<Self> {
        Self::capture_sources(root, pin, &spec.sources)
    }
    fn capture_sources(root: &Path, pin: &str, sources: &[String]) -> Result<Self> {
        Self::capture_sources_with_identity(
            root,
            pin,
            sources,
            &current_host(),
            &current_toolchain()?,
        )
    }
    fn capture_sources_with_identity(
        root: &Path,
        pin: &str,
        sources: &[String],
        host: &str,
        toolchain: &str,
    ) -> Result<Self> {
        let revision = String::from_utf8(git(root, &["rev-parse", "HEAD"])?)
            .map_err(|e| e.to_string())?
            .trim()
            .to_string();
        // Git's glob pathspecs include tracked deletions and nonignored additions.
        // Positive declarations omit docs/policy by default, but can opt them in.
        let patterns: Vec<String> = sources.iter().map(|p| format!(":(top,glob){p}")).collect();
        let mut args = vec![
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
        ];
        args.extend(patterns.iter().map(String::as_str));
        let paths = git(root, &args)?;
        let mut entries = BTreeMap::new();
        for bytes in paths.split(|b| *b == 0).filter(|b| !b.is_empty()) {
            let path = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
            if excluded(path) {
                continue;
            }
            let p = root.join(path);
            let digest = if p.is_dir() {
                // Gitlinks: include actual commit and dirtiness, never walk their fixtures.
                let state = git(&p, &["rev-parse", "HEAD"])?;
                let dirty = git(&p, &["status", "--porcelain", "--untracked-files=all"])?;
                if !dirty.is_empty() {
                    return Err(format!("dirty gitlink cannot supply evidence: {path}"));
                }
                hash(&state)
            } else if p.exists() {
                sha_file(root, path)?
            } else {
                "deleted".into()
            };
            entries.insert(path.to_string(), digest);
        }
        let environment = [
            "RUSTFLAGS",
            "CARGO_ENCODED_RUSTFLAGS",
            "RUSTUP_TOOLCHAIN",
            "CARGO_BUILD_TARGET",
            "CARGO_HOME",
            "RUSTUP_HOME",
            "S04_RUSTUP_HOME",
            "S04_CACHE_HOME",
            "RUSTC_WRAPPER",
            "GOFLAGS",
            "GOOS",
            "GOARCH",
            "CGO_ENABLED",
        ]
        .into_iter()
        .map(|k| (k.into(), std::env::var(k).unwrap_or_default()))
        .collect();
        Ok(Self {
            revision,
            source_sha256: hash(&serde_json::to_vec(&entries).unwrap()),
            upstream_pin: pin.into(),
            environment,
            host: host.into(),
            toolchain: toolchain.into(),
        })
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpstreamManifest {
    schema_version: u32,
    pin: String,
    ledger_generated_sha256: String,
    inventory_sha256: String,
}
pub fn ledger_generated_hash(root: &Path) -> Result<String> {
    let ledger: toml::Value =
        toml::from_str(&fs::read_to_string(root.join("PORTS.toml")).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    fn text(value: &toml::Value, field: &str) -> Result<String> {
        value
            .get(field)
            .and_then(toml::Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .ok_or_else(|| format!("missing/invalid generated field {field}"))
    }
    fn hex(value: &str, n: usize) -> bool {
        value.len() == n
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }
    let pin = text(&ledger, "pin")?;
    if !hex(&pin, 40) {
        return Err("invalid full ledger pin".into());
    }
    let files = ledger
        .get("file")
        .and_then(toml::Value::as_array)
        .ok_or("missing generated file array")?;
    let mut projected = BTreeMap::new();
    for file in files {
        let mut entry = BTreeMap::<String, serde_json::Value>::new();
        for key in ["go", "package", "crate", "kind", "pin", "source_hash"] {
            let value = text(file, key)?;
            if (key == "pin" && !hex(&value, 40)) || (key == "source_hash" && !hex(&value, 64)) {
                return Err(format!("invalid generated {key}"));
            }
            if key == "kind"
                && !matches!(
                    value.as_str(),
                    "source" | "generated" | "harness" | "out-of-scope"
                )
            {
                return Err("invalid generated kind".into());
            }
            entry.insert(key.into(), value.into());
        }
        for key in ["phase", "loc"] {
            let value = file
                .get(key)
                .and_then(toml::Value::as_integer)
                .filter(|n| *n >= 0)
                .ok_or_else(|| format!("invalid nonnegative generated {key}"))?;
            entry.insert(key.into(), value.into());
        }
        let path = text(file, "go")?;
        if projected.insert(path.clone(), entry).is_some() {
            return Err(format!("duplicate generated source path {path}"));
        }
    }
    // Canonical ordering must not depend on another workspace crate enabling
    // serde_json's preserve_order feature through Cargo feature unification.
    let canonical = BTreeMap::from([
        ("pin", serde_json::Value::String(pin)),
        (
            "file",
            serde_json::to_value(projected.into_values().collect::<Vec<_>>())
                .map_err(|e| e.to_string())?,
        ),
    ]);
    Ok(hash(
        &serde_json::to_vec(&canonical).map_err(|e| e.to_string())?,
    ))
}
pub fn provenance(root: &Path, pin: &str) -> Result<()> {
    let m: UpstreamManifest = serde_json::from_slice(&read(&root.join("data/upstream.json"))?)
        .map_err(|e| e.to_string())?;
    if m.schema_version != 2
        || m.pin != pin
        || pin.len() != 40
        || !pin.bytes().all(|c| c.is_ascii_hexdigit())
    {
        return Err("upstream manifest pin/version mismatch".into());
    }
    if ledger_generated_hash(root)? != m.ledger_generated_sha256
        || sha_file(root, "data/go-functions.tsv")? != m.inventory_sha256
    {
        return Err(
            "generated ledger fields/inventory changed: regenerate their upstream manifest".into(),
        );
    }
    let inventory = read(&root.join("data/go-functions.tsv"))?;
    if !inventory.starts_with(format!("# upstream {pin}\n").as_bytes()) {
        return Err("inventory pin does not match ledger".into());
    }
    Ok(())
}
pub fn upstream_ready(root: &Path, pin: &str) -> bool {
    let check = || -> Result<bool> {
        let entry = String::from_utf8(git(root, &["ls-files", "--stage", "--", "upstream"])?)
            .map_err(|e| e.to_string())?;
        let mut fields = entry.split_whitespace();
        if fields.next() != Some("160000") || fields.next() != Some(pin) {
            return Ok(false);
        }
        let url = String::from_utf8(git(
            root,
            &[
                "config",
                "--file",
                ".gitmodules",
                "--get",
                "submodule.upstream.url",
            ],
        )?)
        .map_err(|e| e.to_string())?;
        if url.trim().trim_end_matches(".git") != "https://github.com/microsoft/TypeScript" {
            return Ok(false);
        }
        let sub = root.join("upstream");
        let head =
            String::from_utf8(git(&sub, &["rev-parse", "HEAD"])?).map_err(|e| e.to_string())?;
        Ok(head.trim() == pin
            && git(&sub, &["status", "--porcelain", "--untracked-files=all"])?.is_empty())
    };
    check().unwrap_or(false)
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunSpec {
    command: Vec<String>,
    inputs: Vec<String>,
    #[serde(default = "default_sources")]
    sources: Vec<String>,
    target: String,
    config: String,
    #[serde(default)]
    cases: Option<String>,
}
fn default_sources() -> Vec<String> {
    [
        "crates/**",
        "Cargo.toml",
        "Cargo.lock",
        ".cargo/**",
        "rust-toolchain*",
        "xtask/**",
        "scripts/**",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}
fn valid_source_pattern(pattern: &str) -> bool {
    !pattern.is_empty()
        && !pattern.starts_with('/')
        && !pattern.starts_with(':')
        && !pattern.contains('\\')
        && !pattern.split('/').any(|p| p == ".." || p == ".")
}
pub fn specs(root: &Path) -> Result<BTreeMap<String, RunSpec>> {
    let text = fs::read_to_string(root.join("status/runs.toml")).map_err(|e| e.to_string())?;
    let specs: BTreeMap<String, RunSpec> = toml::from_str(&text).map_err(|e| e.to_string())?;
    for (id, s) in &specs {
        if !valid_id(id)
            || s.command.is_empty()
            || s.target.is_empty()
            || s.config.is_empty()
            || s.inputs.is_empty()
            || s.sources.is_empty()
            || s.sources.iter().any(|p| !valid_source_pattern(p))
        {
            return Err(format!("invalid run declaration: {id}"));
        }
    }
    Ok(specs)
}
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
fn input_hashes(root: &Path, spec: &RunSpec) -> Result<BTreeMap<String, String>> {
    spec.inputs
        .iter()
        .chain(spec.cases.iter())
        .map(|p| Ok((p.clone(), sha_file(root, p)?)))
        .collect()
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema_version: u32,
    run_id: String,
    recorded_at: u64,
    context: Context,
    spec_sha256: String,
    inputs: BTreeMap<String, String>,
    command: Vec<String>,
    target: String,
    config: String,
    host: String,
    toolchain: String,
    exit_code: i32,
    stdout: String,
    stderr: String,
    stdout_sha256: String,
    stderr_sha256: String,
    valid_capture: bool,
}

/// Historical snapshots must keep their original artifacts, even when their
/// inputs are stale. Validate identity and bytes, without regrading old runs.
pub fn validate_history(root: &Path) -> Result<()> {
    let path = root.join("status/history.jsonl");
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("history.jsonl: {error}")),
    };
    let mut checked = BTreeSet::new();
    for (index, line) in text.lines().enumerate() {
        let mut check = || -> Result<()> {
            let snapshot: serde_json::Value =
                serde_json::from_str(line).map_err(|e| e.to_string())?;
            let Some(artifacts) = snapshot.get("evidence_artifacts") else {
                return Ok(()); // Early snapshots predate content-addressed evidence.
            };
            let artifacts = artifacts.as_object().ok_or("invalid evidence_artifacts")?;
            for (id, value) in artifacts {
                let path = value.as_str().ok_or("invalid artifact path")?;
                let digest = path
                    .strip_prefix("status/evidence/")
                    .and_then(|name| name.strip_suffix(".json"))
                    .filter(|digest| {
                        digest.len() == 64
                            && digest
                                .bytes()
                                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    })
                    .ok_or("invalid historical artifact path")?;
                if !checked.insert((id.to_owned(), path.to_owned())) {
                    continue;
                }
                let bytes = read(&root.join(path))?;
                if hash(&bytes) != digest {
                    return Err(format!("historical artifact checksum mismatch: {path}"));
                }
                let record: Record = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                if record.schema_version != 1
                    || record.run_id != *id
                    || record.stdout_sha256 != hash(record.stdout.as_bytes())
                    || record.stderr_sha256 != hash(record.stderr.as_bytes())
                {
                    return Err(format!(
                        "historical record identity/contents mismatch: {path}"
                    ));
                }
            }
            Ok(())
        };
        check().map_err(|e| format!("history.jsonl:{}: {e}", index + 1))?;
    }
    Ok(())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProducerReport {
    #[serde(deserialize_with = "unique_map")]
    metrics: BTreeMap<String, serde_json::Value>,
    #[serde(default, deserialize_with = "unique_map")]
    tests: BTreeMap<String, String>,
}
fn unique_map<'de, D, V>(deserializer: D) -> std::result::Result<BTreeMap<String, V>, D::Error>
where
    D: serde::Deserializer<'de>,
    V: Deserialize<'de>,
{
    struct Visitor<V>(std::marker::PhantomData<V>);
    impl<'de, V: Deserialize<'de>> serde::de::Visitor<'de> for Visitor<V> {
        type Value = BTreeMap<String, V>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a map with unique keys")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut map: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut result = BTreeMap::new();
            while let Some((key, value)) = map.next_entry::<String, V>()? {
                if result.insert(key.clone(), value).is_some() {
                    return Err(serde::de::Error::custom(format!("duplicate key: {key}")));
                }
            }
            Ok(result)
        }
    }
    deserializer.deserialize_map(Visitor(std::marker::PhantomData))
}
fn report_metrics(root: &Path, spec: &RunSpec, stdout: &str) -> Result<BTreeMap<String, Metric>> {
    let report: ProducerReport = serde_json::from_str(stdout)
        .map_err(|e| format!("producer must write one JSON report: {e}"))?;
    let mut metrics = BTreeMap::new();
    for (name, value) in report.metrics {
        if !valid_id(&name) {
            return Err(format!("invalid metric name: {name}"));
        }
        let metric = if let Some(b) = value.as_bool() {
            Metric::Bool(b)
        } else if let Some(n) = value.as_f64().filter(|n| n.is_finite()) {
            Metric::Num(n)
        } else {
            return Err(format!("metric {name} must be a finite number or boolean"));
        };
        metrics.insert(name, metric);
    }
    if let Some(path) = &spec.cases {
        let cases: Vec<String> =
            serde_json::from_slice(&read(&root.join(path))?).map_err(|e| e.to_string())?;
        let expected: BTreeSet<_> = cases.iter().cloned().collect();
        if expected.is_empty()
            || expected.len() != cases.len()
            || expected != report.tests.keys().cloned().collect()
        {
            return Err(
                "test results must cover the entire nonempty frozen manifest exactly once".into(),
            );
        }
        let mut counts = [0.0; 3];
        for status in report.tests.values() {
            counts[match status.as_str() {
                "pass" => 0,
                "fail" => 1,
                "skip" => 2,
                _ => return Err(format!("unknown test status {status}")),
            }] += 1.0;
        }
        for (name, value) in [
            ("tests_total", cases.len() as f64),
            ("tests_passed", counts[0]),
            ("tests_failed", counts[1]),
            ("tests_skipped", counts[2]),
            ("parity", counts[0] / cases.len() as f64),
        ] {
            if metrics.insert(name.into(), Metric::Num(value)).is_some() {
                return Err(format!("producer cannot override derived metric {name}"));
            }
        }
    } else if !report.tests.is_empty() {
        return Err("test results require a declared cases manifest".into());
    }
    Ok(metrics)
}
fn spec_hash(spec: &RunSpec) -> String {
    hash(&serde_json::to_vec(spec).unwrap())
}
struct RunLock(std::path::PathBuf);
impl Drop for RunLock {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.0);
    }
}
pub fn run(root: &Path, id: &str, pin: &str) -> Result<bool> {
    provenance(root, pin)?;
    let all = specs(root)?;
    let spec = all
        .get(id)
        .ok_or_else(|| format!("no run declaration {id}"))?;
    let evidence_dir = root.join("status/evidence");
    fs::create_dir_all(&evidence_dir).map_err(|e| e.to_string())?;
    let lock_path = evidence_dir.join(format!("{id}.lock"));
    fs::create_dir(&lock_path)
        .map_err(|e| format!("cannot lock run {id} (another or interrupted attempt): {e}"))?;
    let _lock = RunLock(lock_path);
    // A failed rerun must never leave the earlier successful attempt current.
    let dir = root.join("status/evidence");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    fs::write(dir.join(format!("{id}.latest")), "incomplete attempt").map_err(|e| e.to_string())?;
    let before = Context::capture_run(root, pin, spec)?;
    let inputs = input_hashes(root, spec)?;
    let output = Command::new(&spec.command[0])
        .args(&spec.command[1..])
        .current_dir(root)
        .output()
        .map_err(|e| e.to_string())?;
    let stdout = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let after = Context::capture_run(root, pin, spec)?;
    let stable = after.same_inputs(&before)
        && after.host == before.host
        && after.toolchain == before.toolchain
        && input_hashes(root, spec)? == inputs;
    let parsed = report_metrics(root, spec, &stdout);
    let success = output.status.success() && stable && parsed.is_ok();
    let host = before.host.clone();
    let toolchain = before.toolchain.clone();
    let record = Record {
        schema_version: 1,
        run_id: id.into(),
        recorded_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        context: before,
        spec_sha256: spec_hash(spec),
        inputs,
        command: spec.command.clone(),
        target: spec.target.clone(),
        config: spec.config.clone(),
        host,
        toolchain,
        exit_code: output.status.code().unwrap_or(-1),
        stdout_sha256: hash(stdout.as_bytes()),
        stderr_sha256: hash(stderr.as_bytes()),
        stdout,
        stderr,
        valid_capture: success,
    };
    let bytes = serde_json::to_vec_pretty(&record).unwrap();
    let digest = hash(&bytes);
    let dir = root.join("status/evidence");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    fs::write(dir.join(format!("{digest}.json")), bytes).map_err(|e| e.to_string())?;
    fs::write(dir.join(format!("{id}.latest")), &digest).map_err(|e| e.to_string())?;
    println!(
        "{}: {} ({})",
        id,
        if success { "recorded" } else { "failed" },
        dir.join(format!("{digest}.json")).display()
    );
    if !success {
        eprintln!("{}", record.stderr);
        if !stable {
            eprintln!("source/inputs changed during run");
        }
        if let Err(e) = parsed {
            eprintln!("{e}");
        }
    }
    Ok(success)
}
#[derive(Default)]
pub struct Loaded {
    pub metrics: Metrics,
    pub states: BTreeMap<String, String>,
    pub artifacts: BTreeMap<String, String>,
}
pub fn load(root: &Path, context: &Context) -> Result<Loaded> {
    load_in_environment(root, context, None)
}

/// Replay evidence against a committed view's renderer identity, never for live gates.
pub fn load_archived(root: &Path, context: &Context) -> Result<Loaded> {
    load_in_environment(root, context, Some(&context.environment))
}

fn load_in_environment(
    root: &Path,
    context: &Context,
    environment: Option<&BTreeMap<String, String>>,
) -> Result<Loaded> {
    // The report captures the host and rustc once. Reuse its selected identity for
    // every run, including archive replay of a view that recorded stale evidence.
    if context.host.is_empty() || context.toolchain.trim().is_empty() {
        return Err("missing report host or Rust toolchain identity".into());
    }
    let mut loaded = Loaded::default();
    for (id, spec) in specs(root)? {
        let latest = root.join(format!("status/evidence/{id}.latest"));
        if !latest.exists() {
            loaded.states.insert(id, "missing".into());
            continue;
        }
        let mut artifact = None;
        let mut validation = || -> Result<(String, BTreeMap<String, Metric>)> {
            let digest = fs::read_to_string(&latest).map_err(|e| e.to_string())?;
            if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("invalid artifact digest".into());
            }
            let path = format!("status/evidence/{digest}.json");
            let bytes = read(&root.join(&path))?;
            if hash(&bytes) != digest {
                return Err("artifact checksum mismatch".into());
            }
            let r: Record = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            if r.schema_version != 1 || r.run_id != id {
                return Err("record identity/version mismatch".into());
            }
            artifact = Some(path.clone());
            let mut current = Context::capture_sources_with_identity(
                root,
                &context.upstream_pin,
                &spec.sources,
                &context.host,
                &context.toolchain,
            )?;
            if let Some(environment) = environment {
                current.environment.clone_from(environment);
            }
            if !r.context.same_inputs(&current)
                || r.spec_sha256 != spec_hash(&spec)
                || r.inputs != input_hashes(root, &spec)?
            {
                return Err("stale: source, pin, command or inputs changed".into());
            }
            if r.host != context.host || r.toolchain != context.toolchain {
                return Err("stale: host or Rust toolchain changed".into());
            }
            if r.exit_code != 0 || !r.valid_capture {
                return Err("failed producer/capture".into());
            }
            if r.stdout_sha256 != hash(r.stdout.as_bytes())
                || r.stderr_sha256 != hash(r.stderr.as_bytes())
                || r.command != spec.command
                || r.target != spec.target
                || r.config != spec.config
                || (!r.context.host.is_empty() && r.context.host != r.host)
                || (!r.context.toolchain.is_empty() && r.context.toolchain != r.toolchain)
            {
                return Err("record contents mismatch".into());
            }
            Ok((path, report_metrics(root, &spec, &r.stdout)?))
        };
        let validation = validation();
        if let Some(path) = artifact {
            loaded.artifacts.insert(id.clone(), path);
        }
        match validation {
            Ok((path, metrics)) => {
                loaded.states.insert(id.clone(), "current".into());
                loaded.artifacts.insert(id.clone(), path);
                for (key, value) in metrics {
                    loaded.metrics.insert(format!("run.{id}.{key}"), value);
                }
            }
            Err(error) => {
                loaded.states.insert(id, error);
            }
        }
    }
    Ok(loaded)
}

#[cfg(test)]
#[path = "evidence_tests.rs"]
mod tests;
